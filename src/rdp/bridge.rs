//! Safe wrapper around the C bridge. All `unsafe` of the RDP engine lives in this file.

use super::auth::{extract_auth_code, gateway_bearer_token, PendingAuth};
use super::clipboard::{
    self, LocalClipboard, LocalFileEntry, RemoteClipboard, RemoteFileEntry, CF_DIB,
    FILECONTENTS_RANGE, FILECONTENTS_SIZE, FILE_LIST_FORMAT_ID,
};
use super::ffi;
use super::input::{self, LockKeys, MouseButton, Viewport};
use crate::error::{AppError, Result};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::fs;
use std::io::{self, Write};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Connection state reported by the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    /// Informational progress (connecting, reconnecting, ...).
    Progress(String),
    /// The session is active (also after an automatic reconnect).
    Connected,
    /// The connection attempt failed.
    Failed(String),
    /// An established session ended.
    Ended(String),
}

/// Events delivered to the frontend. They arrive on FreeRDP threads; frontends forward
/// them to their UI thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeEvent {
    /// New pixels are available via [`NativeRdpBridge::with_frame`]. Coalesced: no further
    /// event is sent until the frame has been read.
    FrameUpdated,
    Status(SessionStatus),
    /// Entra ID sign-in for the session host is required. Show `url` and answer with
    /// [`NativeRdpBridge::provide_auth_result`].
    AuthRequired {
        url: String,
    },
    /// Content copied in the remote session. Files are downloaded on demand with
    /// [`NativeRdpBridge::remote_files`].
    RemoteClipboard(RemoteClipboard),
    /// The host's certificate could not be verified automatically. The connection waits
    /// (up to 5 minutes) for [`NativeRdpBridge::answer_certificate`].
    CertificatePrompt(CertificatePrompt),
    /// Latest network measurements with the session host.
    Network(NetworkStats),
}

/// Server certificate that needs the user's decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificatePrompt {
    pub host: String,
    pub port: u16,
    pub common_name: String,
    pub subject: String,
    pub issuer: String,
    pub fingerprint: String,
    /// The host presented a different certificate than the one trusted before.
    pub changed: bool,
    pub previous_fingerprint: Option<String>,
}

/// Answer to a [`CertificatePrompt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateDecision {
    Reject,
    AcceptOnce,
    AcceptAlways,
}

/// Time to wait for each answer of the host during a file download.
const FILE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Bytes requested per CB_FILECONTENTS_REQUEST during a download.
const FILE_CHUNK_SIZE: u32 = 1024 * 1024;

/// Network characteristics measured by RDP auto-detection; `0` means "not measured".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetworkStats {
    pub average_rtt_ms: u32,
    pub base_rtt_ms: u32,
    pub bandwidth_kbps: u32,
}

/// Coarse connection quality shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionQuality {
    Excellent,
    Good,
    Fair,
    Poor,
    Unknown,
}

impl ConnectionQuality {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Excellent => "Excellent",
            Self::Good => "Good",
            Self::Fair => "Fair",
            Self::Poor => "Poor",
            Self::Unknown => "Measuring",
        }
    }
}

impl NetworkStats {
    /// Rates the connection from latency and (when measured) bandwidth.
    #[must_use]
    pub fn quality(&self) -> ConnectionQuality {
        let rtt = if self.average_rtt_ms > 0 {
            self.average_rtt_ms
        } else {
            self.base_rtt_ms
        };
        if rtt == 0 && self.bandwidth_kbps == 0 {
            return ConnectionQuality::Unknown;
        }
        let by_rtt = match rtt {
            0..=50 => ConnectionQuality::Excellent,
            51..=100 => ConnectionQuality::Good,
            101..=200 => ConnectionQuality::Fair,
            _ => ConnectionQuality::Poor,
        };
        let by_bandwidth = match self.bandwidth_kbps {
            1..=1_499 => ConnectionQuality::Poor,
            1_500..=4_999 => ConnectionQuality::Fair,
            5_000..=19_999 => ConnectionQuality::Good,
            _ => ConnectionQuality::Excellent,
        };
        // The worse of the two wins (variants are declared best to worst).
        if (by_rtt as u8) >= (by_bandwidth as u8) {
            by_rtt
        } else {
            by_bandwidth
        }
    }

    /// One-line description, e.g. "Good · 42 ms · 25.0 Mbps".
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = vec![self.quality().label().to_owned()];
        let rtt = if self.average_rtt_ms > 0 {
            self.average_rtt_ms
        } else {
            self.base_rtt_ms
        };
        if rtt > 0 {
            parts.push(format!("{rtt} ms"));
        }
        if self.bandwidth_kbps > 0 {
            parts.push(format!(
                "{:.1} Mbps",
                f64::from(self.bandwidth_kbps) / 1000.0
            ));
        }
        parts.join(" · ")
    }
}

/// One local monitor for [`NativeRdpBridge::request_layout`], in device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

impl MonitorRect {
    /// Size as the bridge sends it (MS-RDPEDISP limits, even width).
    #[must_use]
    pub fn remote_size(&self) -> (u32, u32) {
        (
            self.width.clamp(MIN_MONITOR_SIZE, MAX_MONITOR_SIZE) & !1,
            self.height.clamp(MIN_MONITOR_SIZE, MAX_MONITOR_SIZE),
        )
    }

    /// Where each monitor appears in the remote frame once the host applied the layout:
    /// the frame covers the bounding box of all monitors. Returns the viewports (in input
    /// order) and the expected frame size.
    #[must_use]
    pub fn viewports(monitors: &[Self]) -> (Vec<Viewport>, (u32, u32)) {
        let min_x = monitors.iter().map(|m| i64::from(m.x)).min().unwrap_or(0);
        let min_y = monitors.iter().map(|m| i64::from(m.y)).min().unwrap_or(0);
        let mut max = (0_i64, 0_i64);
        let viewports = monitors
            .iter()
            .map(|m| {
                let (width, height) = m.remote_size();
                let x = i64::from(m.x) - min_x;
                let y = i64::from(m.y) - min_y;
                max.0 = max.0.max(x + i64::from(width));
                max.1 = max.1.max(y + i64::from(height));
                Viewport {
                    x: u32::try_from(x).unwrap_or(0),
                    y: u32::try_from(y).unwrap_or(0),
                    width,
                    height,
                }
            })
            .collect();
        let size = (
            u32::try_from(max.0).unwrap_or(0),
            u32::try_from(max.1).unwrap_or(0),
        );
        (viewports, size)
    }
}

/// `DISPLAY_CONTROL_MIN_MONITOR_WIDTH/HEIGHT` and the bridge's `MAX_MONITOR_SIZE`.
const MIN_MONITOR_SIZE: u32 = 200;
const MAX_MONITOR_SIZE: u32 = 8192;

pub type EventSink = Arc<dyn Fn(BridgeEvent) + Send + Sync + 'static>;

#[derive(Debug, Clone, Default)]
pub struct BridgeOptions {
    /// Account pre-filled on the session host sign-in page.
    pub login_hint: Option<String>,
}

/// The remote desktop surface (BGRX, 4 bytes per pixel).
#[derive(Debug, Default, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: Vec<u8>,
}

impl Frame {
    /// Applies an update from the engine: a full copy when the geometry changed,
    /// otherwise only the dirty rectangle. Returns whether anything changed.
    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        src: &[u8],
        width: u32,
        height: u32,
        stride: u32,
        rect: (i32, i32, i32, i32),
    ) -> bool {
        if width == 0
            || height == 0
            || (stride as usize) < width as usize * 4
            || src.len() < stride as usize * height as usize
        {
            return false;
        }
        let src = &src[..stride as usize * height as usize];
        if self.width != width
            || self.height != height
            || self.stride != stride
            || self.pixels.len() != src.len()
        {
            self.width = width;
            self.height = height;
            self.stride = stride;
            self.pixels.clear();
            self.pixels.extend_from_slice(src);
            return true;
        }
        let (x, y, w, h) = rect;
        let clamp = |v: i64, max: u32| -> usize {
            usize::try_from(v.clamp(0, i64::from(max))).unwrap_or(0)
        };
        let (x0, x1) = (
            clamp(i64::from(x), width),
            clamp(i64::from(x) + i64::from(w), width),
        );
        let (y0, y1) = (
            clamp(i64::from(y), height),
            clamp(i64::from(y) + i64::from(h), height),
        );
        if x0 >= x1 || y0 >= y1 {
            return false;
        }
        let stride = stride as usize;
        for row in y0..y1 {
            let range = row * stride + x0 * 4..row * stride + x1 * 4;
            self.pixels[range.clone()].copy_from_slice(&src[range]);
        }
        true
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Context pointer usable from [`RemoteFileFetcher`]s.
#[derive(Clone, Copy)]
struct CtxRef(NonNull<ffi::RdpBridgeContext>);

// SAFETY: only used for thread-safe C entry points (see NativeRdpBridge), and only while
// held through `Shared::ctx`, which Drop clears before the context is freed.
unsafe impl Send for CtxRef {}
unsafe impl Sync for CtxRef {}

/// Local clipboard content offered to the host.
#[derive(Default)]
struct LocalState {
    /// Incremented on every change; invalidates `Shared::format_data`.
    generation: u64,
    content: Option<Arc<LocalClipboard>>,
    /// File list last sent to the host (index = FILEDESCRIPTOR index).
    files: Arc<Vec<LocalFileEntry>>,
}

/// File list on the remote clipboard.
#[derive(Default)]
struct RemoteState {
    /// Incremented whenever the remote clipboard changes or the session ends.
    generation: u64,
    files: Option<Arc<Vec<RemoteFileEntry>>>,
}

/// Format data encoded for the size query of a two-phase clipboard request.
struct EncodedFormat {
    generation: u64,
    format_id: u32,
    data: Vec<u8>,
}

/// State shared with the FreeRDP threads.
struct Shared {
    sink: EventSink,
    options: BridgeOptions,
    frame: Mutex<Frame>,
    frame_pending: AtomicBool,
    pending_auth: Mutex<Option<PendingAuth>>,
    local: Mutex<LocalState>,
    format_data: Mutex<Option<EncodedFormat>>,
    remote: Mutex<RemoteState>,
    /// Pending file contents requests by stream id; dropping a sender cancels the request.
    transfers: Mutex<HashMap<u32, SyncSender<Option<Vec<u8>>>>>,
    next_stream_id: AtomicU32,
    ended: AtomicBool,
    ctx: RwLock<Option<CtxRef>>,
    /// Answers file contents requests in tests instead of the C bridge.
    #[cfg(test)]
    fake_host: Mutex<Option<FakeHost>>,
}

#[cfg(test)]
type FakeHost = Box<dyn Fn(&Shared, u32, u32, u32, u64, u32) -> bool + Send + Sync>;

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Shared>();
};

impl Shared {
    fn new(options: BridgeOptions, sink: EventSink) -> Self {
        Self {
            sink,
            options,
            frame: Mutex::default(),
            frame_pending: AtomicBool::new(false),
            pending_auth: Mutex::new(None),
            local: Mutex::default(),
            format_data: Mutex::new(None),
            remote: Mutex::default(),
            transfers: Mutex::default(),
            next_stream_id: AtomicU32::new(1),
            ended: AtomicBool::new(false),
            ctx: RwLock::new(None),
            #[cfg(test)]
            fake_host: Mutex::new(None),
        }
    }

    /// Runs `f` with the live C context, or returns `None` once the bridge is dropped.
    fn with_ctx<R>(&self, f: impl FnOnce(*mut ffi::RdpBridgeContext) -> R) -> Option<R> {
        let ctx = self.ctx.read().unwrap_or_else(PoisonError::into_inner);
        ctx.map(|c| f(c.0.as_ptr()))
    }

    fn handle_frame(
        &self,
        src: &[u8],
        width: u32,
        height: u32,
        stride: u32,
        rect: (i32, i32, i32, i32),
    ) {
        let changed = lock(&self.frame).update(src, width, height, stride, rect);
        if changed && !self.frame_pending.swap(true, Ordering::AcqRel) {
            (self.sink)(BridgeEvent::FrameUpdated);
        }
    }

    fn begin_auth(&self, auth: PendingAuth) {
        let url = auth.authorize_url(self.options.login_hint.as_deref());
        *lock(&self.pending_auth) = Some(auth);
        (self.sink)(BridgeEvent::AuthRequired { url });
    }

    /// Turns the redirect URL from the sign-in UI into the token request body for FreeRDP.
    fn finish_auth(&self, redirect_url: Option<&str>) -> Option<String> {
        let auth = lock(&self.pending_auth).take()?;
        let code = extract_auth_code(redirect_url?);
        if code.is_none() {
            log::warn!("No authorization code in the sign-in redirect");
        }
        Some(auth.token_request(&code?))
    }

    fn handle_status(&self, code: c_int, message: String) {
        let status = match code {
            ffi::STATUS_CONNECTED => SessionStatus::Connected,
            ffi::STATUS_FAILED => SessionStatus::Failed(message),
            ffi::STATUS_ENDED => SessionStatus::Ended(message),
            ffi::STATUS_PROGRESS => SessionStatus::Progress(message),
            other => {
                log::debug!("Unknown bridge status {other}: {message}");
                SessionStatus::Progress(message)
            }
        };
        if matches!(status, SessionStatus::Ended(_) | SessionStatus::Failed(_)) {
            self.end_transfers();
        }
        (self.sink)(BridgeEvent::Status(status));
    }

    /// Fails downloads in progress and forgets the remote file list.
    fn end_transfers(&self) {
        self.ended.store(true, Ordering::Release);
        self.invalidate_remote_files();
    }

    fn invalidate_remote_files(&self) {
        {
            let mut remote = lock(&self.remote);
            remote.generation += 1;
            remote.files = None;
        }
        // After the generation bump: a request registered later sees the new generation.
        lock(&self.transfers).clear();
    }

    /// Replaces the local clipboard content; returns false when it is unchanged.
    fn set_local(&self, content: Option<LocalClipboard>) -> bool {
        let mut local = lock(&self.local);
        if local.content.as_deref() == content.as_ref() {
            return false;
        }
        local.generation += 1;
        local.content = content.map(Arc::new);
        local.files = Arc::default();
        true
    }

    fn handle_remote_clipboard(&self, kind: c_int, data: &[u8]) {
        let (content, local) = match kind {
            ffi::CLIP_CHANGED => {
                // The host owns the clipboard now; what was offered before is stale.
                self.invalidate_remote_files();
                self.set_local(None);
                return;
            }
            ffi::CLIP_TEXT => {
                let text = clipboard::decode_utf16le(data);
                if text.is_empty() {
                    return;
                }
                (
                    RemoteClipboard::Text(text.clone()),
                    LocalClipboard::Text(text),
                )
            }
            ffi::CLIP_DIB | ffi::CLIP_DIBV5 => {
                let Some(image) = clipboard::decode_dib(data) else {
                    log::warn!("Unsupported bitmap on the remote clipboard");
                    return;
                };
                (
                    RemoteClipboard::Image(image.clone()),
                    LocalClipboard::Image(image),
                )
            }
            ffi::CLIP_FILE_LIST => {
                let Some(entries) = clipboard::decode_file_list(data).filter(|e| !e.is_empty())
                else {
                    log::warn!("Unusable file list on the remote clipboard");
                    return;
                };
                let files = entries.iter().map(|e| e.file.clone()).collect();
                lock(&self.remote).files = Some(Arc::new(entries));
                (self.sink)(BridgeEvent::RemoteClipboard(RemoteClipboard::Files(files)));
                return;
            }
            other => {
                log::debug!("Unknown remote clipboard kind {other}");
                return;
            }
        };
        // The frontend copies this to the local clipboard; remembering it here keeps the
        // resulting local "changed" notification from being announced back.
        self.set_local(Some(local));
        (self.sink)(BridgeEvent::RemoteClipboard(content));
    }

    /// Encodes the local clipboard for a format the host requested.
    fn encode_local(&self, format_id: u32) -> Option<(u64, Vec<u8>)> {
        let (generation, content) = {
            let local = lock(&self.local);
            (local.generation, local.content.clone()?)
        };
        let data = match (&*content, format_id) {
            (LocalClipboard::Text(text), _) => clipboard::encode_clipboard_text(text, format_id)?,
            (LocalClipboard::Image(image), CF_DIB) => clipboard::encode_dib(image)?,
            (LocalClipboard::Files(paths), FILE_LIST_FORMAT_ID) => {
                // Walked without holding locks; discarded if the clipboard changed meanwhile.
                let entries = clipboard::collect_local_files(paths);
                let data = clipboard::encode_file_list(&entries)?;
                let mut local = lock(&self.local);
                if local.generation != generation {
                    return None;
                }
                local.files = Arc::new(entries);
                data
            }
            _ => return None,
        };
        Some((generation, data))
    }

    /// Two-phase format data request: the size query (no buffer) encodes and caches the
    /// data, the second call copies it.
    fn clipboard_request(&self, format_id: u32, buffer: Option<&mut [u8]>) -> usize {
        let cached = lock(&self.format_data).take();
        let current = lock(&self.local).generation;
        let (generation, data) = match cached {
            Some(c) if c.generation == current && c.format_id == format_id => {
                (c.generation, c.data)
            }
            _ => match self.encode_local(format_id) {
                Some(encoded) => encoded,
                None => return 0,
            },
        };
        let len = data.len();
        match buffer {
            Some(buffer) if buffer.len() >= len => buffer[..len].copy_from_slice(&data),
            _ => {
                *lock(&self.format_data) = Some(EncodedFormat {
                    generation,
                    format_id,
                    data,
                });
            }
        }
        len
    }

    fn file_request(&self, list_index: u32, flags: u32, position: u64, buf: &mut [u8]) -> i64 {
        let files = Arc::clone(&lock(&self.local).files);
        let Some(entry) = files.get(list_index as usize) else {
            return -1;
        };
        match clipboard::read_local_file(entry, flags, position, buf) {
            Ok(n) => i64::try_from(n).unwrap_or(-1),
            Err(e) => {
                log::warn!("Could not serve a clipboard file to the host: {}", e.kind());
                -1
            }
        }
    }

    fn file_contents(&self, stream_id: u32, data: Option<&[u8]>) {
        if let Some(sender) = lock(&self.transfers).remove(&stream_id) {
            let _ = sender.try_send(data.map(<[u8]>::to_vec));
        }
    }

    fn remote_file_list(&self) -> Option<(u64, Arc<Vec<RemoteFileEntry>>)> {
        let remote = lock(&self.remote);
        Some((remote.generation, Arc::clone(remote.files.as_ref()?)))
    }

    fn check_remote(&self, generation: u64) -> io::Result<()> {
        if self.ended.load(Ordering::Acquire) {
            Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "the remote session ended",
            ))
        } else if lock(&self.remote).generation != generation {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "the remote clipboard changed",
            ))
        } else {
            Ok(())
        }
    }

    /// Sends one CB_FILECONTENTS_REQUEST and waits for the answer.
    fn request_file_contents(
        &self,
        generation: u64,
        list_index: u32,
        flags: u32,
        position: u64,
        size: u32,
    ) -> io::Result<Vec<u8>> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let stream_id = self.next_stream_id.fetch_add(1, Ordering::Relaxed);
        lock(&self.transfers).insert(stream_id, sender);
        let forget = || {
            lock(&self.transfers).remove(&stream_id);
        };
        if let Err(e) = self.check_remote(generation) {
            forget();
            return Err(e);
        }
        if !self.send_file_request(stream_id, list_index, flags, position, size) {
            forget();
            self.check_remote(generation)?;
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "clipboard file transfer is not available",
            ));
        }
        match receiver.recv_timeout(FILE_REQUEST_TIMEOUT) {
            Ok(Some(data)) => Ok(data),
            Ok(None) => Err(io::Error::other("the remote PC refused the file request")),
            Err(RecvTimeoutError::Timeout) => {
                forget();
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the remote PC did not answer",
                ))
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.check_remote(generation)?;
                Err(io::Error::other("file transfer cancelled"))
            }
        }
    }

    fn send_file_request(
        &self,
        stream_id: u32,
        list_index: u32,
        flags: u32,
        position: u64,
        size: u32,
    ) -> bool {
        #[cfg(test)]
        if let Some(fake) = lock(&self.fake_host).as_ref() {
            return fake(self, stream_id, list_index, flags, position, size);
        }
        // SAFETY: ctx is live while `Shared::ctx` holds it (see with_ctx).
        self.with_ctx(|ctx| unsafe {
            ffi::freerdp_bridge_request_file_contents(
                ctx, stream_id, list_index, flags, position, size,
            )
        })
        .unwrap_or(false)
    }

    fn download_file(&self, generation: u64, list_index: u32, target: &Path) -> io::Result<()> {
        let size = self.request_file_contents(generation, list_index, FILECONTENTS_SIZE, 0, 8)?;
        let size = size
            .get(..8)
            .and_then(|b| b.try_into().ok())
            .map(u64::from_le_bytes)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid file size"))?;
        let parent = target
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid target"))?;
        let temp = parent.join(format!(".wal-{}.part", uuid::Uuid::new_v4().simple()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let result = (|| {
            let mut position = 0u64;
            while position < size {
                let wanted = u32::try_from((size - position).min(u64::from(FILE_CHUNK_SIZE)))
                    .unwrap_or(FILE_CHUNK_SIZE);
                let chunk = self.request_file_contents(
                    generation,
                    list_index,
                    FILECONTENTS_RANGE,
                    position,
                    wanted,
                )?;
                if chunk.is_empty() || chunk.len() > wanted as usize {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unexpected file chunk from the remote PC",
                    ));
                }
                file.write_all(&chunk)?;
                position += chunk.len() as u64;
            }
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, target)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    fn fetch_remote_files(&self, dest: &Path) -> io::Result<Vec<PathBuf>> {
        let (generation, entries) = self.remote_file_list().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no files on the remote clipboard")
        })?;
        self.check_remote(generation)?;
        fs::create_dir_all(dest)?;
        let root = fs::canonicalize(dest)?;
        let mut top_level: Vec<PathBuf> = Vec::new();
        for entry in entries.iter() {
            self.check_remote(generation)?;
            let target = clipboard::safe_target(&root, &entry.file.path, entry.file.is_dir)?;
            if !entry.file.is_dir {
                self.download_file(generation, entry.index, &target)?;
            }
            let first = entry.file.path.split('/').next().unwrap_or_default();
            let top = root.join(first);
            if !top_level.contains(&top) {
                top_level.push(top);
            }
        }
        self.check_remote(generation)?;
        Ok(top_level)
    }
}

/// Downloads the files of the remote clipboard (see [`NativeRdpBridge::remote_files`]).
#[derive(Clone)]
pub struct RemoteFileFetcher {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for RemoteFileFetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteFileFetcher").finish_non_exhaustive()
    }
}

impl RemoteFileFetcher {
    /// Blocking (call from a worker thread). Downloads every file/dir of the remote file
    /// list announced most recently into `dest` (created if missing), preserving the
    /// relative structure, and returns the paths of the top-level entries. Fails if the
    /// session ended, the remote clipboard changed meanwhile, a request failed, or no file
    /// list is available. Never writes outside `dest`; existing files are replaced.
    pub fn fetch(&self, dest: &Path) -> io::Result<Vec<PathBuf>> {
        self.shared.fetch_remote_files(dest)
    }
}

/// Runs a callback body, never letting a panic unwind into C.
fn guard<R>(fallback: R, f: impl FnOnce() -> R) -> R {
    std::panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        log::error!("Panic in FreeRDP callback");
        fallback
    })
}

/// # Safety
/// `ptr` must be null or the `Shared` pointer registered in [`NativeRdpBridge::new`],
/// which stays alive until after `freerdp_bridge_free` returns.
unsafe fn shared_ref<'a>(ptr: *mut c_void) -> Option<&'a Shared> {
    unsafe { ptr.cast::<Shared>().cast_const().as_ref() }
}

/// # Safety
/// `ptr` must be null or point to a NUL-terminated string valid for the duration of the call.
unsafe fn opt_string(ptr: *const c_char) -> Option<String> {
    (!ptr.is_null()).then(|| {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    })
}

#[allow(clippy::too_many_arguments)]
extern "C" fn on_frame(
    user_data: *mut c_void,
    pixels: *const u8,
    width: u32,
    height: u32,
    stride: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    if pixels.is_null() || width == 0 || height == 0 {
        return;
    }
    guard((), || {
        let len = stride as usize * height as usize;
        // SAFETY: user_data is ours; FreeRDP's GDI primary buffer holds `stride * height` bytes.
        let (shared, src) = unsafe {
            (
                shared_ref(user_data),
                std::slice::from_raw_parts(pixels, len),
            )
        };
        if let Some(shared) = shared {
            shared.handle_frame(src, width, height, stride, (x, y, w, h));
        }
    });
}

extern "C" fn on_auth(
    user_data: *mut c_void,
    client_id: *const c_char,
    scope: *const c_char,
    req_cnf: *const c_char,
) {
    guard((), || {
        // SAFETY: user_data is ours; the strings are NUL-terminated and valid during the call.
        let (shared, client_id, scope, req_cnf) = unsafe {
            (
                shared_ref(user_data),
                opt_string(client_id),
                opt_string(scope),
                opt_string(req_cnf),
            )
        };
        if let (Some(shared), Some(scope)) = (shared, scope) {
            shared.begin_auth(PendingAuth::new(client_id, scope, req_cnf));
        }
    });
}

extern "C" fn on_status(user_data: *mut c_void, status_code: c_int, message: *const c_char) {
    guard((), || {
        // SAFETY: user_data is ours; message is NUL-terminated and valid during the call.
        let (shared, message) = unsafe { (shared_ref(user_data), opt_string(message)) };
        if let Some(shared) = shared {
            shared.handle_status(status_code, message.unwrap_or_default());
        }
    });
}

/// # Safety
/// `data` must be null or valid for `len` readable bytes during the call.
unsafe fn opt_bytes<'a>(data: *const u8, len: usize) -> &'a [u8] {
    if data.is_null() || len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    }
}

extern "C" fn on_clipboard(user_data: *mut c_void, kind: c_int, data: *const u8, len: usize) {
    guard((), || {
        // SAFETY: user_data is ours; the format data response holds `len` readable bytes.
        let (shared, bytes) = unsafe { (shared_ref(user_data), opt_bytes(data, len)) };
        if let Some(shared) = shared {
            shared.handle_remote_clipboard(kind, bytes);
        }
    });
}

extern "C" fn on_certificate(user_data: *mut c_void, cert: *const ffi::RdpBridgeCertificate) {
    guard((), || {
        // SAFETY: user_data is ours; `cert` and its strings are valid during the call.
        let (shared, cert) = unsafe { (shared_ref(user_data), cert.as_ref()) };
        let (Some(shared), Some(cert)) = (shared, cert) else {
            return;
        };
        // SAFETY: the strings are NUL-terminated (or NULL) and valid during the call.
        let text = |ptr| unsafe { opt_string(ptr) };
        let prompt = CertificatePrompt {
            host: text(cert.host).unwrap_or_default(),
            port: cert.port,
            common_name: text(cert.common_name).unwrap_or_default(),
            subject: text(cert.subject).unwrap_or_default(),
            issuer: text(cert.issuer).unwrap_or_default(),
            fingerprint: text(cert.fingerprint).unwrap_or_default(),
            changed: cert.changed,
            previous_fingerprint: if cert.changed {
                text(cert.old_fingerprint)
            } else {
                None
            },
        };
        (shared.sink)(BridgeEvent::CertificatePrompt(prompt));
    });
}

extern "C" fn on_file_request(
    user_data: *mut c_void,
    list_index: u32,
    flags: u32,
    position: u64,
    buffer: *mut u8,
    buffer_len: usize,
) -> i64 {
    guard(-1, || {
        // SAFETY: user_data is ours.
        let Some(shared) = (unsafe { shared_ref(user_data) }) else {
            return -1;
        };
        let buf: &mut [u8] = if buffer.is_null() || buffer_len == 0 {
            &mut []
        } else {
            // SAFETY: the C caller provides `buffer` valid for `buffer_len` writable bytes.
            unsafe { std::slice::from_raw_parts_mut(buffer, buffer_len) }
        };
        shared.file_request(list_index, flags, position, buf)
    })
}

extern "C" fn on_file_contents(
    user_data: *mut c_void,
    stream_id: u32,
    ok: bool,
    data: *const u8,
    len: usize,
) {
    guard((), || {
        // SAFETY: user_data is ours; the response holds `len` readable bytes.
        let (shared, bytes) = unsafe { (shared_ref(user_data), opt_bytes(data, len)) };
        if let Some(shared) = shared {
            shared.file_contents(stream_id, ok.then_some(bytes));
        }
    });
}

extern "C" fn on_network(
    user_data: *mut c_void,
    average_rtt_ms: u32,
    base_rtt_ms: u32,
    bandwidth_kbps: u32,
) {
    guard((), || {
        // SAFETY: user_data is ours.
        if let Some(shared) = unsafe { shared_ref(user_data) } {
            (shared.sink)(BridgeEvent::Network(NetworkStats {
                average_rtt_ms,
                base_rtt_ms,
                bandwidth_kbps,
            }));
        }
    });
}

extern "C" fn on_clipboard_request(
    user_data: *mut c_void,
    format_id: u32,
    buffer: *mut u8,
    buffer_len: usize,
) -> usize {
    guard(0, || {
        // SAFETY: user_data is ours.
        let Some(shared) = (unsafe { shared_ref(user_data) }) else {
            return 0;
        };
        let buffer = (!buffer.is_null() && buffer_len > 0).then(|| {
            // SAFETY: the C caller provides `buffer` valid for `buffer_len` writable bytes.
            unsafe { std::slice::from_raw_parts_mut(buffer, buffer_len) }
        });
        shared.clipboard_request(format_id, buffer)
    })
}

/// Context pointer handed to the teardown thread.
struct ContextPtr(NonNull<ffi::RdpBridgeContext>);

// SAFETY: the C bridge context may be freed from any thread.
unsafe impl Send for ContextPtr {}

impl ContextPtr {
    fn free(self) {
        // SAFETY: the pointer came from freerdp_bridge_create and is freed exactly once.
        unsafe { ffi::freerdp_bridge_free(self.0.as_ptr()) }
    }
}

static TEARDOWNS: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

/// Waits up to `timeout` for sessions that are still disconnecting in the background.
/// Call before the process exits so servers see a clean disconnect. Returns `true`
/// when all sessions have been torn down.
pub fn join_pending_teardowns(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = {
            let mut handles = lock(&TEARDOWNS);
            let (done, pending): (Vec<_>, Vec<_>) = std::mem::take(&mut *handles)
                .into_iter()
                .partition(JoinHandle::is_finished);
            *handles = pending;
            for handle in done {
                let _ = handle.join();
            }
            handles.len()
        };
        if remaining == 0 {
            return true;
        }
        if Instant::now() >= deadline {
            log::warn!("{remaining} RDP session(s) still disconnecting at exit");
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// FreeRDP engine version.
#[must_use]
pub fn version() -> String {
    // SAFETY: returns a static NUL-terminated string (or NULL).
    unsafe { opt_string(ffi::freerdp_bridge_version()) }.unwrap_or_default()
}

/// First FreeRDP release that understands the graphics-pipeline surface protection and
/// watermark commands Windows sends when screen-capture protection is enabled.
const SURFACE_PROTECTION_VERSION: (u32, u32) = (3, 27);

/// `major.minor` of a FreeRDP version string such as `3.15.0` or `3.27.1-dev`.
fn parse_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.trim().split(['.', '-', '+']);
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Whether the engine's graphics pipeline (`/gfx`) can handle hosts with screen-capture
/// protection. Older FreeRDP drops the whole pipeline on the first protected surface,
/// leaving a blank window until the host disconnects.
#[must_use]
pub fn supports_surface_protection() -> bool {
    parse_version(&version()).is_some_and(|v| v >= SURFACE_PROTECTION_VERSION)
}

/// Whether the engine was built with H.264 graphics-pipeline support (`/gfx:AVC444`).
/// Distribution packages built without FFmpeg or OpenH264 (e.g. Ubuntu) reject AVC modes.
#[must_use]
pub fn supports_h264() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        // SAFETY: returns a static NUL-terminated string (or NULL).
        let config = unsafe { opt_string(ffi::freerdp_bridge_build_config()) };
        config.is_some_and(|c| build_option_enabled(&c, "WITH_GFX_H264"))
    })
}

/// Whether the engine includes the USB redirection channel (`urbdrc`). Builds without it,
/// such as vcpkg's default `freerdp[client]`, reject `/usb` and every session using it.
#[must_use]
pub fn supports_usb_redirection() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    // SAFETY: the option is a NUL-terminated string that outlives the call.
    *SUPPORTED.get_or_init(|| unsafe { ffi::freerdp_bridge_accepts_option(c"/usb:auto".as_ptr()) })
}

/// Whether `option` is `ON` in a FreeRDP build configuration string.
fn build_option_enabled(config: &str, option: &str) -> bool {
    config
        .split_whitespace()
        .filter_map(|entry| entry.split_once('='))
        .any(|(key, value)| key == option && value.eq_ignore_ascii_case("ON"))
}

/// One embedded FreeRDP session.
///
/// Input methods may be called from any thread and are ignored while not connected.
/// Dropping the bridge disconnects in the background (see [`join_pending_teardowns`]);
/// no events are delivered once teardown has finished.
pub struct NativeRdpBridge {
    ctx: NonNull<ffi::RdpBridgeContext>,
    // Shared with the C side as a raw pointer; must outlive `ctx` (see Drop).
    shared: Arc<Shared>,
}

// SAFETY: every C entry point used through `&self` is thread-safe (input/work locks in the
// bridge), and the context is freed exactly once in Drop.
unsafe impl Send for NativeRdpBridge {}
unsafe impl Sync for NativeRdpBridge {}

impl NativeRdpBridge {
    /// Creates a session from a FreeRDP argument vector (argv[0] included, see
    /// [`crate::session::build_freerdp_args`]). Call [`Self::start`] to connect.
    pub fn new(args: &[String], options: BridgeOptions, sink: EventSink) -> Result<Self> {
        if args.is_empty() {
            return Err(AppError::Session("no FreeRDP arguments".into()));
        }
        let cstrings = args
            .iter()
            .map(|arg| CString::new(arg.as_str()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| AppError::Session("FreeRDP argument contains a NUL byte".into()))?;
        let argv: Vec<*const c_char> = cstrings.iter().map(|cs| cs.as_ptr()).collect();
        let argc = c_int::try_from(argv.len())
            .map_err(|_| AppError::Session("too many FreeRDP arguments".into()))?;
        let gateway_token = gateway_bearer_token(args)
            .map(CString::new)
            .transpose()
            .map_err(|_| AppError::Session("invalid gateway token".into()))?;

        let shared = Arc::new(Shared::new(options, sink));
        let callbacks = ffi::RdpBridgeCallbacks {
            on_frame,
            on_auth,
            on_status,
            on_clipboard,
            on_clipboard_request,
            on_network,
            on_certificate,
            on_file_request,
            on_file_contents,
            user_data: Arc::as_ptr(&shared).cast_mut().cast(),
        };

        // SAFETY: all pointers are valid for the call (C copies strings and callbacks), the
        // callbacks match the C signatures, and `shared` outlives the context (see Drop).
        let raw = unsafe {
            ffi::freerdp_bridge_create(
                argv.as_ptr(),
                argc,
                gateway_token
                    .as_ref()
                    .map_or(std::ptr::null(), |t| t.as_ptr()),
                &raw const callbacks,
            )
        };
        let ctx = NonNull::new(raw)
            .ok_or_else(|| AppError::Session("FreeRDP rejected the connection settings".into()))?;
        *shared.ctx.write().unwrap_or_else(PoisonError::into_inner) = Some(CtxRef(ctx));
        Ok(Self { ctx, shared })
    }

    fn ctx(&self) -> *mut ffi::RdpBridgeContext {
        self.ctx.as_ptr()
    }

    /// Starts connecting on a FreeRDP thread. Progress arrives as [`BridgeEvent::Status`].
    pub fn start(&self) -> Result<()> {
        // SAFETY (all methods): ctx is a live context from freerdp_bridge_create.
        if unsafe { ffi::freerdp_bridge_start(self.ctx()) } {
            Ok(())
        } else {
            Err(AppError::Session(
                "could not start the FreeRDP session".into(),
            ))
        }
    }

    /// Completes a pending session-host sign-in with the redirect URL captured by the
    /// sign-in UI, or cancels it (`None`).
    pub fn provide_auth_result(&self, redirect_url: Option<&str>) {
        let request = self
            .shared
            .finish_auth(redirect_url)
            .and_then(|body| CString::new(body).ok());
        let ptr = request.as_ref().map_or(std::ptr::null(), |r| r.as_ptr());
        unsafe { ffi::freerdp_bridge_provide_token_request(self.ctx(), ptr) }
    }

    /// Calls `f` with the current remote frame and re-arms [`BridgeEvent::FrameUpdated`].
    pub fn with_frame<R>(&self, f: impl FnOnce(&Frame) -> R) -> R {
        self.shared.frame_pending.store(false, Ordering::Release);
        f(&lock(&self.shared.frame))
    }

    /// Size of the remote desktop, `(0, 0)` before the first frame.
    #[must_use]
    pub fn frame_size(&self) -> (u32, u32) {
        let frame = lock(&self.shared.frame);
        (frame.width, frame.height)
    }

    pub fn send_pointer_move(&self, x: u16, y: u16) {
        unsafe { ffi::freerdp_bridge_send_mouse(self.ctx(), input::PTR_FLAGS_MOVE, x, y) }
    }

    pub fn send_button(&self, button: MouseButton, down: bool, x: u16, y: u16) {
        let (flags, extended) = button.flags(down);
        unsafe {
            if extended {
                ffi::freerdp_bridge_send_extended_mouse(self.ctx(), flags, x, y);
            } else {
                ffi::freerdp_bridge_send_mouse(self.ctx(), flags, x, y);
            }
        }
    }

    /// Sends a wheel rotation (multiples of [`input::WHEEL_DELTA`] per notch;
    /// positive = up for vertical, right for horizontal).
    pub fn send_wheel(&self, rotation: i32, horizontal: bool, x: u16, y: u16) {
        for flags in input::wheel_flags(rotation, horizontal) {
            unsafe { ffi::freerdp_bridge_send_mouse(self.ctx(), flags, x, y) }
        }
    }

    /// Sends a key by RDP scancode (see [`super::keymap`]).
    pub fn send_key(&self, rdp_scancode: u32, down: bool, repeat: bool) {
        unsafe { ffi::freerdp_bridge_send_scancode(self.ctx(), down, repeat, rdp_scancode) }
    }

    /// Sends a UTF-16 code unit as a Unicode keyboard event (TS_UNICODE_KEYBOARD_EVENT),
    /// for text that has no scancode on the remote layout. Characters outside the BMP
    /// are sent as two code units (surrogate pair).
    pub fn send_unicode(&self, code_unit: u16, down: bool) {
        unsafe { ffi::freerdp_bridge_send_unicode(self.ctx(), down, code_unit) }
    }

    /// Answers the pending [`BridgeEvent::CertificatePrompt`].
    pub fn answer_certificate(&self, decision: CertificateDecision) {
        let decision = match decision {
            CertificateDecision::Reject => ffi::CERT_REJECT,
            CertificateDecision::AcceptOnce => ffi::CERT_ACCEPT_ONCE,
            CertificateDecision::AcceptAlways => ffi::CERT_ACCEPT,
        };
        unsafe { ffi::freerdp_bridge_answer_certificate(self.ctx(), decision) }
    }

    /// Synchronizes lock keys and releases remote modifiers when the window gains focus.
    pub fn send_focus_in(&self, locks: LockKeys) {
        unsafe { ffi::freerdp_bridge_send_focus_in(self.ctx(), locks.sync_flags()) }
    }

    /// Asks the session host to resize the desktop to `width`×`height` device pixels at
    /// `scale_percent` desktop scale.
    pub fn request_resize(&self, width: u32, height: u32, scale_percent: u32) {
        let desktop_scale = crate::session::clamp_scale(scale_percent);
        let device_scale = crate::session::device_scale_for(desktop_scale);
        unsafe {
            ffi::freerdp_bridge_request_resize(
                self.ctx(),
                width,
                height,
                desktop_scale,
                device_scale,
            );
        }
    }

    /// Asks the session host for a multi-monitor desktop (one remote monitor per entry).
    /// The primary monitor is placed at the origin; at most 16 monitors are used.
    pub fn request_layout(&self, monitors: &[MonitorRect], scale_percent: u32) {
        let layout: Vec<ffi::RdpBridgeMonitor> = monitors
            .iter()
            .take(ffi::MAX_MONITORS)
            .map(|m| ffi::RdpBridgeMonitor {
                left: m.x,
                top: m.y,
                width: m.width,
                height: m.height,
                primary: m.primary,
            })
            .collect();
        let Ok(count) = u32::try_from(layout.len()) else {
            return;
        };
        if count == 0 {
            return;
        }
        let desktop_scale = crate::session::clamp_scale(scale_percent);
        let device_scale = crate::session::device_scale_for(desktop_scale);
        unsafe {
            ffi::freerdp_bridge_request_layout(
                self.ctx(),
                layout.as_ptr(),
                count,
                desktop_scale,
                device_scale,
            );
        }
    }

    /// Offers local clipboard content to the remote session (announces the matching
    /// formats; the host pulls the data when pasting). Unchanged content is not announced
    /// again; empty text, an empty file list or an invalid image clears the offer.
    pub fn set_local_clipboard(&self, content: LocalClipboard) {
        let kind = match &content {
            _ if !content.is_offerable() => ffi::LOCAL_NONE,
            LocalClipboard::Text(_) => ffi::LOCAL_TEXT,
            LocalClipboard::Image(_) => ffi::LOCAL_IMAGE,
            LocalClipboard::Files(_) => ffi::LOCAL_FILES,
        };
        let content = (kind != ffi::LOCAL_NONE).then_some(content);
        if self.shared.set_local(content) {
            unsafe { ffi::freerdp_bridge_announce_clipboard(self.ctx(), kind) }
        }
    }

    /// Handle for downloading the current remote file list; usable from any thread and
    /// after the bridge is dropped (requests then fail).
    #[must_use]
    pub fn remote_files(&self) -> RemoteFileFetcher {
        RemoteFileFetcher {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl Drop for NativeRdpBridge {
    fn drop(&mut self) {
        // Waits for fetcher calls into C, then keeps them from using the context again.
        *self
            .shared
            .ctx
            .write()
            .unwrap_or_else(PoisonError::into_inner) = None;
        self.shared.end_transfers();
        let ctx = ContextPtr(self.ctx);
        let shared = Arc::clone(&self.shared);
        // Disconnecting joins FreeRDP threads and can take seconds; never block the UI.
        let spawned = std::thread::Builder::new()
            .name("rdp-teardown".into())
            .spawn(move || {
                ctx.free();
                drop(shared);
            });
        match spawned {
            Ok(handle) => lock(&TEARDOWNS).push(handle),
            Err(e) => {
                log::warn!("Could not spawn teardown thread ({e}); disconnecting inline");
                ContextPtr(self.ctx).free();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rdp::clipboard::{CF_TEXT, CF_UNICODETEXT};
    use std::ffi::CString;
    use std::sync::atomic::AtomicUsize;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn null_sink() -> EventSink {
        Arc::new(|_| {})
    }

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn bridge_creation_with_args() {
        let _guard = lock(&TEST_LOCK);
        let bridge = NativeRdpBridge::new(
            &args(&["freerdp", "/v:127.0.0.1", "/cert:ignore"]),
            BridgeOptions::default(),
            null_sink(),
        );
        assert!(bridge.is_ok());
        drop(bridge);
        assert!(join_pending_teardowns(Duration::from_secs(10)));
    }

    #[test]
    fn bridge_creation_with_gateway_token() {
        let _guard = lock(&TEST_LOCK);
        let bridge = NativeRdpBridge::new(
            &args(&[
                "freerdp",
                "/v:127.0.0.1",
                "/gateway:type:arm,bearer:test_token_avd_bearer_12345",
                "/cert:ignore",
            ]),
            BridgeOptions::default(),
            null_sink(),
        );
        assert!(bridge.is_ok());
    }

    #[test]
    fn freerdp_accepts_session_arguments() {
        let _guard = lock(&TEST_LOCK);
        let launch = crate::api::RdpLaunch {
            rdp_file: "full address:s:127.0.0.1\r\nusername:s:u@contoso.com\r\n".into(),
            gateway_token: "tok".into(),
        };
        let opts = crate::session::SessionOptions::from_config(
            &crate::AppConfig::default(),
            1280,
            720,
            150,
            Some("u@contoso.com".into()),
        );
        let prepared = crate::session::prepare(&launch, &opts).unwrap();
        let bridge = NativeRdpBridge::new(&prepared.args, BridgeOptions::default(), null_sink());
        assert!(
            bridge.is_ok(),
            "{:?}",
            crate::session::redact_args(&prepared.args)
        );
    }

    #[test]
    fn freerdp_accepts_all_optional_features() {
        let _guard = lock(&TEST_LOCK);
        let config = crate::AppConfig {
            usb_devices: vec!["046d:c52b".into()],
            enable_smartcard: true,
            enable_printers: true,
            enable_drive_redirection: true,
            shared_folder_path: std::env::temp_dir().display().to_string(),
            ..crate::AppConfig::default()
        };
        for mode in ["auto", "rfx"] {
            let config = crate::AppConfig {
                graphics_mode: mode.into(),
                ..config.clone()
            };
            let opts = crate::session::SessionOptions::from_config(&config, 1280, 720, 100, None);
            let connection = crate::connections::SavedConnection::new("", "127.0.0.1", "").unwrap();
            let args = crate::session::build_direct_args(
                &connection,
                &crate::session::Credentials::default(),
                &opts,
            )
            .unwrap();
            let bridge = NativeRdpBridge::new(&args, BridgeOptions::default(), null_sink());
            assert!(bridge.is_ok(), "{args:?}");
        }
    }

    #[test]
    fn freerdp_accepts_drive_paths_ending_in_backslash() {
        let _guard = lock(&TEST_LOCK);
        // On Windows a directory path with a trailing separator; elsewhere a directory
        // whose name ends in a backslash.
        let dir = std::env::temp_dir().join(format!("wal-drive-{}\\", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = crate::AppConfig {
            enable_drive_redirection: true,
            shared_folder_path: dir.display().to_string(),
            ..crate::AppConfig::default()
        };
        let opts = crate::session::SessionOptions::from_config(&config, 1280, 720, 100, None);
        let connection = crate::connections::SavedConnection::new("", "127.0.0.1", "").unwrap();
        let args = crate::session::build_direct_args(
            &connection,
            &crate::session::Credentials::default(),
            &opts,
        )
        .unwrap();
        let result = NativeRdpBridge::new(&args, BridgeOptions::default(), null_sink());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            args.iter().any(|a| a.starts_with("/drive:Shared,")),
            "{args:?}"
        );
        assert!(result.is_ok(), "{args:?}");
    }

    #[test]
    fn freerdp_accepts_direct_session_arguments() {
        let _guard = lock(&TEST_LOCK);
        for address in ["127.0.0.1:3390", "[::1]", "localhost"] {
            let connection =
                crate::connections::SavedConnection::new("", address, "CORP\\me").unwrap();
            let opts = crate::session::SessionOptions::from_config(
                &crate::AppConfig::default(),
                1280,
                720,
                175,
                Some(connection.username.clone()),
            );
            let credentials = crate::session::Credentials {
                password: Some("p:w".into()),
                gateway_password: None,
            };
            let prepared =
                crate::session::prepare_direct(&connection, &credentials, &opts).unwrap();
            let bridge =
                NativeRdpBridge::new(&prepared.args, BridgeOptions::default(), null_sink());
            assert!(bridge.is_ok(), "{prepared:?}");
        }
    }

    #[test]
    fn freerdp_accepts_advanced_direct_arguments() {
        use crate::connections::{
            AuthMethod, CertificatePolicy, Gateway, RemoteApp, SavedConnection, SecurityProtocol,
        };
        use crate::session::{Credentials, Quality};
        let _guard = lock(&TEST_LOCK);
        let base = SavedConnection {
            domain: "CORP".into(),
            gateway: Some(Gateway {
                address: "gw.example.com:8443".into(),
                username: "gwuser".into(),
                separate_credentials: true,
            }),
            admin_session: true,
            remote_app: Some(RemoteApp {
                program: "||notepad".into(),
                arguments: "C:\\file.txt".into(),
            }),
            ..SavedConnection::new("", "pc.example.com", "me").unwrap()
        };
        let credentials = Credentials {
            password: Some("pw".into()),
            gateway_password: Some("gw-pw".into()),
        };
        for (i, quality) in Quality::ALL.into_iter().enumerate() {
            let connection = SavedConnection {
                security: SecurityProtocol::ALL[i % SecurityProtocol::ALL.len()],
                certificate: CertificatePolicy::ALL[i % CertificatePolicy::ALL.len()],
                auth: AuthMethod::ALL[i % AuthMethod::ALL.len()],
                gateway: (i % 2 == 0).then(|| base.gateway.clone()).flatten(),
                ..base.clone()
            };
            let config = crate::AppConfig {
                quality: quality.id().into(),
                enable_drive_redirection: true,
                shared_folder_path: std::env::temp_dir().display().to_string(),
                extra_folders: vec![crate::config::SharedFolder {
                    name: "Extra".into(),
                    path: std::env::temp_dir().display().to_string(),
                }],
                hotplug_drives: true,
                ..crate::AppConfig::default()
            };
            let opts = crate::session::SessionOptions::from_config(&config, 1280, 720, 100, None);
            let args = crate::session::build_direct_args(&connection, &credentials, &opts).unwrap();
            let bridge = NativeRdpBridge::new(&args, BridgeOptions::default(), null_sink());
            assert!(bridge.is_ok(), "{:?}", crate::session::redact_args(&args));
        }
    }

    #[test]
    fn bridge_rejects_bad_arguments() {
        let _guard = lock(&TEST_LOCK);
        assert!(NativeRdpBridge::new(&[], BridgeOptions::default(), null_sink()).is_err());
        assert!(NativeRdpBridge::new(
            &args(&["freerdp", "/v:a\0b"]),
            BridgeOptions::default(),
            null_sink()
        )
        .is_err());
        assert!(NativeRdpBridge::new(
            &args(&["freerdp", "/v:h", "/not-a-real-option"]),
            BridgeOptions::default(),
            null_sink()
        )
        .is_err());
    }

    #[test]
    fn build_options_are_parsed() {
        let config = "WITH_FFMPEG=OFF WITH_GFX_H264=ON WITH_GFX_H264_AVAILABLE=1";
        assert!(build_option_enabled(config, "WITH_GFX_H264"));
        assert!(!build_option_enabled(config, "WITH_FFMPEG"));
        assert!(!build_option_enabled(config, "WITH_OPENH264"));
        assert!(!build_option_enabled("WITH_GFX_H264=OFF", "WITH_GFX_H264"));
        let _ = supports_h264();
    }

    #[test]
    fn failures_without_error_code_report_the_logged_cause() {
        let _guard = lock(&TEST_LOCK);
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let sink: EventSink = Arc::new(move |event| {
            if let BridgeEvent::Status(SessionStatus::Failed(message)) = event {
                let _ = lock(&tx).send(message);
            }
        });
        // An ARM gateway without load-balance info fails before any network access and
        // leaves FreeRDP's error code at "Success".
        let args = args(&[
            "freerdp",
            "/v:127.0.0.1",
            "/gateway:type:arm,url:wss://127.0.0.1:1",
        ]);
        let bridge = NativeRdpBridge::new(&args, BridgeOptions::default(), sink).unwrap();
        bridge.start().unwrap();
        let message = rx.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_ne!(message, "Success");
        assert!(!message.is_empty());
        eprintln!("failure message: {message}");
    }

    #[test]
    fn probes_command_line_options() {
        // SAFETY: NUL-terminated literals.
        unsafe {
            assert!(ffi::freerdp_bridge_accepts_option(c"/smartcard".as_ptr()));
            assert!(!ffi::freerdp_bridge_accepts_option(
                c"/not-a-real-option".as_ptr()
            ));
            assert!(!ffi::freerdp_bridge_accepts_option(std::ptr::null()));
        }
        let _ = supports_usb_redirection();
    }

    #[test]
    fn version_is_reported() {
        assert!(version().starts_with('3'), "{}", version());
        assert!(parse_version(&version()).is_some(), "{}", version());
    }

    #[test]
    fn parses_engine_versions() {
        assert_eq!(parse_version("3.15.0"), Some((3, 15)));
        assert_eq!(parse_version("3.27.1-dev"), Some((3, 27)));
        assert_eq!(parse_version("4.0"), Some((4, 0)));
        assert_eq!(parse_version("garbage"), None);
        assert!(parse_version("3.15.0").unwrap() < SURFACE_PROTECTION_VERSION);
        assert!(parse_version("3.27.0").unwrap() >= SURFACE_PROTECTION_VERSION);
        assert!(parse_version("4.0.0").unwrap() >= SURFACE_PROTECTION_VERSION);
    }

    #[test]
    fn finish_auth_requires_pending_request() {
        let urls = Arc::new(Mutex::new(Vec::new()));
        let sink_urls = Arc::clone(&urls);
        let shared = Shared::new(
            BridgeOptions {
                login_hint: Some("user@contoso.com".into()),
            },
            Arc::new(move |e| {
                if let BridgeEvent::AuthRequired { url } = e {
                    lock(&sink_urls).push(url);
                }
            }),
        );
        assert_eq!(shared.finish_auth(Some("x://y?code=abc")), None);
        shared.begin_auth(PendingAuth::new(None, "s".into(), None));
        assert!(lock(&urls)[0].ends_with("login_hint=user%40contoso.com"));
        assert!(shared
            .finish_auth(Some("x://y?code=abc"))
            .unwrap()
            .contains("&code=abc&"));
        assert_eq!(
            shared.finish_auth(Some("x://y?code=abc")),
            None,
            "pending auth is consumed"
        );

        shared.begin_auth(PendingAuth::new(None, "s".into(), None));
        assert_eq!(shared.finish_auth(None), None);
        assert!(
            lock(&shared.pending_auth).is_none(),
            "cancel clears the request"
        );
    }

    #[test]
    fn clipboard_request_callback_two_phase() {
        let shared = Shared::new(BridgeOptions::default(), null_sink());
        let ud = std::ptr::from_ref(&shared).cast_mut().cast::<c_void>();
        assert_eq!(
            on_clipboard_request(ud, CF_TEXT, std::ptr::null_mut(), 0),
            0
        );

        shared.set_local(Some(LocalClipboard::Text("hi".into())));
        let needed = on_clipboard_request(ud, CF_UNICODETEXT, std::ptr::null_mut(), 0);
        assert_eq!(needed, 6);
        let mut small = [0xAAu8; 4];
        assert_eq!(
            on_clipboard_request(ud, CF_UNICODETEXT, small.as_mut_ptr(), 4),
            6
        );
        assert_eq!(small, [0xAA; 4], "too-small buffer must not be written");
        let mut buf = vec![0u8; needed];
        assert_eq!(
            on_clipboard_request(ud, CF_UNICODETEXT, buf.as_mut_ptr(), needed),
            6
        );
        assert_eq!(buf, [b'h', 0, b'i', 0, 0, 0]);
    }

    fn recording_shared() -> (Shared, Arc<Mutex<Vec<BridgeEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = Arc::clone(&events);
        let shared = Shared::new(
            BridgeOptions::default(),
            Arc::new(move |e| lock(&sink_events).push(e)),
        );
        (shared, events)
    }

    fn file_list(names: &[(&str, bool)]) -> Vec<u8> {
        let mut data = u32::try_from(names.len()).unwrap().to_le_bytes().to_vec();
        for (name, is_dir) in names {
            let mut desc = [0u8; 592];
            desc[0..4].copy_from_slice(&0x4064u32.to_le_bytes());
            let attributes: u32 = if *is_dir { 0x10 } else { 0x80 };
            desc[36..40].copy_from_slice(&attributes.to_le_bytes());
            for (dst, unit) in desc[72..]
                .as_chunks_mut::<2>()
                .0
                .iter_mut()
                .zip(name.encode_utf16())
            {
                *dst = unit.to_le_bytes();
            }
            data.extend_from_slice(&desc);
        }
        data
    }

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("wal-bridge-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn remote_clipboard_events_and_echo_suppression() {
        let (shared, events) = recording_shared();
        let ud = std::ptr::from_ref(&shared).cast_mut().cast::<c_void>();
        let text: Vec<u8> = "hé"
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        on_clipboard(ud, ffi::CLIP_CHANGED, std::ptr::null(), 0);
        on_clipboard(ud, ffi::CLIP_TEXT, text.as_ptr(), text.len());
        assert_eq!(
            lock(&events).pop(),
            Some(BridgeEvent::RemoteClipboard(RemoteClipboard::Text(
                "hé".into()
            )))
        );
        assert!(
            !shared.set_local(Some(LocalClipboard::Text("hé".into()))),
            "remote text is not announced back"
        );

        let image = clipboard::RgbaImage {
            width: 1,
            height: 1,
            pixels: vec![1, 2, 3, 255],
        };
        let dib = clipboard::encode_dib(&image).unwrap();
        on_clipboard(ud, ffi::CLIP_DIB, dib.as_ptr(), dib.len());
        assert_eq!(
            lock(&events).pop(),
            Some(BridgeEvent::RemoteClipboard(RemoteClipboard::Image(image)))
        );
        on_clipboard(ud, ffi::CLIP_DIBV5, [1u8, 2].as_ptr(), 2);
        assert!(lock(&events).is_empty(), "undecodable bitmap is dropped");

        let list = file_list(&[("dir", true), ("..\\x", false), ("dir\\f.txt", false)]);
        on_clipboard(ud, ffi::CLIP_FILE_LIST, list.as_ptr(), list.len());
        let Some(BridgeEvent::RemoteClipboard(RemoteClipboard::Files(files))) = lock(&events).pop()
        else {
            panic!("expected a file list");
        };
        assert_eq!(
            files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["dir", "dir/f.txt"]
        );
        let (_, entries) = shared.remote_file_list().unwrap();
        assert_eq!(entries[1].index, 2, "original index kept for requests");

        on_clipboard(ud, ffi::CLIP_CHANGED, std::ptr::null(), 0);
        assert!(shared.remote_file_list().is_none());
        assert!(lock(&shared.local).content.is_none());
    }

    #[test]
    fn clipboard_request_serves_images_and_files() {
        let shared = Shared::new(BridgeOptions::default(), null_sink());
        let ud = std::ptr::from_ref(&shared).cast_mut().cast::<c_void>();
        let image = clipboard::RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![9; 8],
        };
        shared.set_local(Some(LocalClipboard::Image(image)));
        assert_eq!(
            on_clipboard_request(ud, CF_TEXT, std::ptr::null_mut(), 0),
            0
        );
        let needed = on_clipboard_request(ud, CF_DIB, std::ptr::null_mut(), 0);
        assert_eq!(needed, 48);
        let mut buf = vec![0u8; needed];
        assert_eq!(
            on_clipboard_request(ud, CF_DIB, buf.as_mut_ptr(), needed),
            needed
        );
        assert_eq!(clipboard::decode_dib(&buf).unwrap().pixels, vec![9; 8]);

        let dir = TestDir::new();
        let file = dir.0.join("a.txt");
        fs::write(&file, b"abcdef").unwrap();
        shared.set_local(Some(LocalClipboard::Files(vec![file])));
        let mut chunk = [0u8; 4];
        assert_eq!(
            on_file_request(ud, 0, FILECONTENTS_RANGE, 0, chunk.as_mut_ptr(), 4),
            -1,
            "no file list sent yet"
        );
        let needed = on_clipboard_request(ud, FILE_LIST_FORMAT_ID, std::ptr::null_mut(), 0);
        assert_eq!(needed, 4 + 592);
        let mut list = vec![0u8; needed];
        on_clipboard_request(ud, FILE_LIST_FORMAT_ID, list.as_mut_ptr(), needed);
        assert_eq!(
            clipboard::decode_file_list(&list).unwrap()[0].file.path,
            "a.txt"
        );
        let mut size = [0u8; 8];
        assert_eq!(
            on_file_request(ud, 0, FILECONTENTS_SIZE, 0, size.as_mut_ptr(), 8),
            8
        );
        assert_eq!(u64::from_le_bytes(size), 6);
        assert_eq!(
            on_file_request(ud, 0, FILECONTENTS_RANGE, 2, chunk.as_mut_ptr(), 4),
            4
        );
        assert_eq!(&chunk, b"cdef");
        assert_eq!(
            on_file_request(ud, 1, FILECONTENTS_RANGE, 0, chunk.as_mut_ptr(), 4),
            -1
        );

        shared.set_local(Some(LocalClipboard::Text("x".into())));
        assert_eq!(
            on_file_request(ud, 0, FILECONTENTS_SIZE, 0, size.as_mut_ptr(), 8),
            -1,
            "new clipboard content withdraws the file list"
        );
    }

    #[test]
    fn certificate_prompt_is_forwarded() {
        let (shared, events) = recording_shared();
        let ud = std::ptr::from_ref(&shared).cast_mut().cast::<c_void>();
        let strings =
            ["host", "cn", "subject", "issuer", "new", "old"].map(|s| CString::new(s).unwrap());
        let cert = ffi::RdpBridgeCertificate {
            host: strings[0].as_ptr(),
            port: 3389,
            common_name: strings[1].as_ptr(),
            subject: strings[2].as_ptr(),
            issuer: strings[3].as_ptr(),
            fingerprint: strings[4].as_ptr(),
            changed: true,
            old_fingerprint: strings[5].as_ptr(),
        };
        on_certificate(ud, &raw const cert);
        assert_eq!(
            lock(&events).pop(),
            Some(BridgeEvent::CertificatePrompt(CertificatePrompt {
                host: "host".into(),
                port: 3389,
                common_name: "cn".into(),
                subject: "subject".into(),
                issuer: "issuer".into(),
                fingerprint: "new".into(),
                changed: true,
                previous_fingerprint: Some("old".into()),
            }))
        );
    }

    /// Serves `files` (by list index) like a host answering CB_FILECONTENTS_REQUEST.
    fn fake_host(files: Vec<Vec<u8>>, max_chunk: usize) -> FakeHost {
        Box::new(move |shared, stream_id, index, flags, position, size| {
            let Some(data) = files.get(index as usize) else {
                shared.file_contents(stream_id, None);
                return true;
            };
            if flags == FILECONTENTS_SIZE {
                assert_eq!((position, size), (0, 8));
                shared.file_contents(stream_id, Some(&(data.len() as u64).to_le_bytes()));
            } else {
                let start = usize::try_from(position).unwrap().min(data.len());
                let end = (start + (size as usize).min(max_chunk)).min(data.len());
                shared.file_contents(stream_id, Some(&data[start..end]));
            }
            true
        })
    }

    #[test]
    fn fetch_downloads_remote_files_into_destination() {
        let (shared, _events) = recording_shared();
        let shared = Arc::new(shared);
        let fetcher = RemoteFileFetcher {
            shared: Arc::clone(&shared),
        };
        let dir = TestDir::new();
        let dest = dir.0.join("dest");
        assert_eq!(
            fetcher.fetch(&dest).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );

        let list = file_list(&[
            ("top", true),
            ("top\\a.bin", false),
            ("top\\sub\\empty", false),
            ("..\\escape", false),
            ("single.txt", false),
        ]);
        shared.handle_remote_clipboard(ffi::CLIP_FILE_LIST, &list);
        assert_eq!(
            fetcher.fetch(&dest).unwrap_err().kind(),
            io::ErrorKind::NotConnected,
            "no session"
        );

        let big: Vec<u8> = (0..(FILE_CHUNK_SIZE as usize * 2 + 10))
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect();
        *lock(&shared.fake_host) = Some(fake_host(
            vec![
                vec![],
                big.clone(),
                vec![],
                b"evil".to_vec(),
                b"hello".to_vec(),
            ],
            700_000,
        ));
        let top = fetcher.fetch(&dest).unwrap();
        let root = fs::canonicalize(&dest).unwrap();
        assert_eq!(top, vec![root.join("top"), root.join("single.txt")]);
        assert_eq!(fs::read(root.join("top/a.bin")).unwrap(), big);
        assert_eq!(fs::read(root.join("top/sub/empty")).unwrap(), b"");
        assert_eq!(fs::read(root.join("single.txt")).unwrap(), b"hello");
        assert!(!dir.0.join("escape").exists());
        let leftovers = fs::read_dir(root.join("top"))
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".part")
            })
            .count();
        assert_eq!(leftovers, 0);

        // A host that fails a request leaves no partial file behind.
        *lock(&shared.fake_host) = Some(Box::new(|shared, stream_id, _, flags, _, _| {
            if flags == FILECONTENTS_SIZE {
                shared.file_contents(stream_id, Some(&100u64.to_le_bytes()));
            } else {
                shared.file_contents(stream_id, None);
            }
            true
        }));
        let other = dir.0.join("other");
        assert!(fetcher.fetch(&other).is_err());
        assert_eq!(fs::read_dir(other.join("top")).unwrap().count(), 0);

        // The remote clipboard changing makes pending downloads fail.
        *lock(&shared.fake_host) = Some(Box::new(|shared, _, _, _, _, _| {
            shared.handle_remote_clipboard(ffi::CLIP_CHANGED, &[]);
            true
        }));
        shared.handle_remote_clipboard(ffi::CLIP_FILE_LIST, &list);
        assert_eq!(
            fetcher.fetch(&dir.0.join("third")).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );

        shared.handle_remote_clipboard(ffi::CLIP_FILE_LIST, &list);
        shared.end_transfers();
        assert_eq!(
            fetcher.fetch(&dir.0.join("fourth")).unwrap_err().kind(),
            io::ErrorKind::NotFound,
            "the list is gone once the session ended"
        );
    }

    #[test]
    fn frame_updates_copy_dirty_rect_and_coalesce_events() {
        let events = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&events);
        let shared = Shared::new(
            BridgeOptions::default(),
            Arc::new(move |e| {
                assert_eq!(e, BridgeEvent::FrameUpdated);
                counter.fetch_add(1, Ordering::SeqCst);
            }),
        );
        // 4x3 surface with 20-byte stride (padding).
        let mut src = vec![1u8; 20 * 3];
        shared.handle_frame(&src, 4, 3, 20, (0, 0, 4, 3));
        assert_eq!(events.load(Ordering::SeqCst), 1);
        assert_eq!(lock(&shared.frame).pixels, src);

        src.fill(2);
        shared.handle_frame(&src, 4, 3, 20, (1, 1, 2, 1));
        assert_eq!(
            events.load(Ordering::SeqCst),
            1,
            "pending frame not re-announced"
        );
        {
            let frame = lock(&shared.frame);
            assert_eq!(frame.pixels[20 + 4..20 + 12], [2; 8]);
            assert_eq!(frame.pixels[20..20 + 4], [1; 4]);
            assert_eq!(frame.pixels[20 + 12..20 + 16], [1; 4]);
            assert!(frame.pixels[..20].iter().all(|&b| b == 1));
        }

        shared.frame_pending.store(false, Ordering::Release);
        shared.handle_frame(&src, 4, 3, 20, (10, 10, 5, 5));
        assert_eq!(
            events.load(Ordering::SeqCst),
            1,
            "out-of-bounds rect is ignored"
        );
        shared.handle_frame(&src, 4, 3, 20, (-2, -2, 100, 100));
        assert_eq!(events.load(Ordering::SeqCst), 2);
        assert_eq!(lock(&shared.frame).pixels[..16], [2; 16]);

        let resized = vec![3u8; 8 * 2 * 4];
        shared.frame_pending.store(false, Ordering::Release);
        shared.handle_frame(&resized, 8, 2, 32, (0, 0, 1, 1));
        let frame = lock(&shared.frame);
        assert_eq!((frame.width, frame.height, frame.stride), (8, 2, 32));
        assert_eq!(
            frame.pixels, resized,
            "geometry change copies the full surface"
        );
    }

    #[test]
    fn network_quality_uses_worst_measurement() {
        let stats = |average_rtt_ms, bandwidth_kbps| NetworkStats {
            average_rtt_ms,
            base_rtt_ms: 0,
            bandwidth_kbps,
        };
        assert_eq!(stats(0, 0).quality(), ConnectionQuality::Unknown);
        assert_eq!(stats(20, 0).quality(), ConnectionQuality::Excellent);
        assert_eq!(stats(20, 3_000).quality(), ConnectionQuality::Fair);
        assert_eq!(stats(150, 50_000).quality(), ConnectionQuality::Fair);
        assert_eq!(stats(300, 50_000).quality(), ConnectionQuality::Poor);
        assert_eq!(stats(42, 25_000).summary(), "Excellent · 42 ms · 25.0 Mbps");
    }

    #[test]
    fn monitor_viewports_cover_the_bounding_box() {
        let monitors = [
            MonitorRect {
                x: 0,
                y: 0,
                width: 2560,
                height: 1440,
                primary: true,
            },
            MonitorRect {
                x: -1921,
                y: 360,
                width: 1921,
                height: 1080,
                primary: false,
            },
        ];
        let (viewports, size) = MonitorRect::viewports(&monitors);
        assert_eq!(size, (1921 + 2560, 1440));
        assert_eq!(
            viewports[0],
            Viewport {
                x: 1921,
                y: 0,
                width: 2560,
                height: 1440
            }
        );
        // Widths are made even like the bridge does.
        assert_eq!(
            viewports[1],
            Viewport {
                x: 0,
                y: 360,
                width: 1920,
                height: 1080
            }
        );
    }
}
