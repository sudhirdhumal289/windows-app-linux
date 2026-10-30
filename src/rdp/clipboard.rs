//! Clipboard encoding for the CLIPRDR channel (MS-RDPECLIP): text, device-independent
//! bitmaps and file lists, plus the local file access used for file transfers.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const CF_TEXT: u32 = 1;
pub(crate) const CF_DIB: u32 = 8;
pub(crate) const CF_UNICODETEXT: u32 = 13;
/// Client format id of "FileGroupDescriptorW" (`RDP_BRIDGE_FILE_LIST_FORMAT_ID`).
pub(crate) const FILE_LIST_FORMAT_ID: u32 = 0xC0B0;

pub(crate) const FILECONTENTS_SIZE: u32 = 0x1;
pub(crate) const FILECONTENTS_RANGE: u32 = 0x2;

const FD_ATTRIBUTES: u32 = 0x4;
const FD_WRITESTIME: u32 = 0x20;
const FD_FILESIZE: u32 = 0x40;
const FD_PROGRESSUI: u32 = 0x4000;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

/// `FILEDESCRIPTORW` size and its `cFileName` capacity (UTF-16 units, NUL included).
const FILE_DESCRIPTOR_LEN: usize = 592;
const FILE_NAME_UNITS: usize = 260;
const MAX_FILE_LIST_ENTRIES: usize = 65_536;
const MAX_DIRECTORY_DEPTH: usize = 64;

/// Largest bitmap side accepted from the host.
const MAX_IMAGE_SIDE: u32 = 16_384;
const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;

/// 100 ns intervals between 1601-01-01 (FILETIME epoch) and 1970-01-01.
const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;

/// RGBA8, top-down, tightly packed (stride = width * 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    fn is_valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && (self.width as usize)
                .checked_mul(self.height as usize)
                .and_then(|n| n.checked_mul(4))
                == Some(self.pixels.len())
    }
}

/// Local clipboard content offered to the remote PC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalClipboard {
    Text(String),
    Image(RgbaImage),
    Files(Vec<PathBuf>),
}

impl LocalClipboard {
    /// Whether there is anything to offer.
    pub(crate) fn is_offerable(&self) -> bool {
        match self {
            Self::Text(text) => !text.is_empty(),
            Self::Image(image) => image.is_valid(),
            Self::Files(paths) => !paths.is_empty(),
        }
    }
}

/// A file or directory on the remote clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    /// Relative, '/'-separated and sanitized.
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
}

/// Remote clipboard content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteClipboard {
    Text(String),
    Image(RgbaImage),
    Files(Vec<RemoteFile>),
}

/// Encodes local clipboard text for a CLIPRDR format (NUL-terminated, as Windows expects).
/// Windows applications expect CRLF line endings on the clipboard; standalone `\n` is normalized to `\r\n`.
pub(crate) fn encode_clipboard_text(text: &str, format_id: u32) -> Option<Vec<u8>> {
    if text.is_empty() {
        return None;
    }
    let normalized = if text.contains('\n') {
        let mut s = String::with_capacity(text.len() + 16);
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\r' {
                s.push('\r');
                if chars.peek() == Some(&'\n') {
                    s.push(chars.next().unwrap());
                } else {
                    s.push('\n');
                }
            } else if c == '\n' {
                s.push('\r');
                s.push('\n');
            } else {
                s.push(c);
            }
        }
        std::borrow::Cow::Owned(s)
    } else {
        std::borrow::Cow::Borrowed(text)
    };
    match format_id {
        CF_UNICODETEXT => Some(
            normalized
                .encode_utf16()
                .chain(std::iter::once(0))
                .flat_map(u16::to_le_bytes)
                .collect(),
        ),
        CF_TEXT => {
            let mut bytes = Vec::with_capacity(normalized.len() + 1);
            bytes.extend_from_slice(normalized.as_bytes());
            bytes.push(0);
            Some(bytes)
        }
        _ => None,
    }
}

/// Decodes CF_UNICODETEXT (UTF-16LE, optionally NUL-terminated) clipboard data.
pub(crate) fn decode_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn i32_at(data: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

/// Encodes an image as CF_DIB: BITMAPINFOHEADER, 32 bpp BI_RGB, bottom-up BGRA rows.
pub(crate) fn encode_dib(image: &RgbaImage) -> Option<Vec<u8>> {
    if !image.is_valid() {
        return None;
    }
    let width = i32::try_from(image.width).ok()?;
    let height = i32::try_from(image.height).ok()?;
    let stride = image.width as usize * 4;
    let size_image = u32::try_from(image.pixels.len()).ok()?;
    let mut out = Vec::with_capacity(40 + image.pixels.len());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&BI_RGB.to_le_bytes());
    out.extend_from_slice(&size_image.to_le_bytes());
    // 2835 pixels per meter = 72 DPI.
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for row in image.pixels.chunks_exact(stride).rev() {
        for [r, g, b, a] in row.as_chunks::<4>().0 {
            out.extend_from_slice(&[*b, *g, *r, *a]);
        }
    }
    Some(out)
}

/// One color channel of a BI_BITFIELDS pixel.
#[derive(Clone, Copy)]
struct Channel {
    mask: u32,
    shift: u32,
    max: u32,
}

impl Channel {
    fn new(mask: u32) -> Self {
        let shift = if mask == 0 { 0 } else { mask.trailing_zeros() };
        let max = mask.checked_shr(shift).unwrap_or(0);
        Self { mask, shift, max }
    }

    fn is_contiguous(self) -> bool {
        self.max & self.max.wrapping_add(1) == 0
    }

    fn extract(self, pixel: u32) -> u8 {
        if self.max == 0 {
            return 0;
        }
        let value = u64::from((pixel & self.mask) >> self.shift);
        u8::try_from(value * 255 / u64::from(self.max)).unwrap_or(u8::MAX)
    }
}

/// Decodes CF_DIB / CF_DIBV5 data (BITMAPINFOHEADER, V4 or V5 header; 24 bpp or 32 bpp
/// BI_RGB, 32 bpp BI_BITFIELDS). Alpha is used only when present and not all zero.
pub(crate) fn decode_dib(data: &[u8]) -> Option<RgbaImage> {
    let header_size = u32_at(data, 0)?;
    if !matches!(header_size, 40 | 52 | 56 | 108 | 124) || data.len() < header_size as usize {
        return None;
    }
    let width = i32_at(data, 4)?;
    let raw_height = i32_at(data, 8)?;
    let bit_count = u16_at(data, 14)?;
    let compression = u32_at(data, 16)?;
    let colors_used = u32_at(data, 32)?;
    let width = u32::try_from(width).ok().filter(|&w| w > 0)?;
    let height = raw_height.checked_abs()?.unsigned_abs();
    let top_down = raw_height < 0;
    if height == 0 || width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE || colors_used > 256 {
        return None;
    }

    let mut offset = header_size as usize;
    let (red, green, blue, alpha) = match (bit_count, compression) {
        (24, BI_RGB) => (0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0),
        (32, BI_RGB) => (0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000),
        (32, BI_BITFIELDS) => {
            // BITMAPINFOHEADER is followed by the masks; larger headers contain them at
            // the same offset.
            if header_size == 40 {
                offset += 12;
            }
            let masks_at = 40;
            let alpha = if header_size >= 56 {
                u32_at(data, 52)?
            } else {
                0
            };
            (
                u32_at(data, masks_at)?,
                u32_at(data, masks_at + 4)?,
                u32_at(data, masks_at + 8)?,
                alpha,
            )
        }
        _ => return None,
    };
    let channels = [red, green, blue, alpha].map(Channel::new);
    if channels.iter().any(|c| !c.is_contiguous()) {
        return None;
    }

    offset = offset.checked_add(colors_used as usize * 4)?;
    let bytes_per_pixel = usize::from(bit_count / 8);
    let stride = (width as usize * bytes_per_pixel).checked_add(3)? & !3;
    let image_len = stride.checked_mul(height as usize)?;
    // Some producers repeat the color masks after a V4/V5 header.
    if compression == BI_BITFIELDS
        && header_size > 40
        && data.len().checked_sub(offset) == Some(image_len + 12)
    {
        offset += 12;
    }
    let pixel_data = data.get(offset..offset.checked_add(image_len)?)?;

    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    let mut any_alpha = false;
    for y in 0..height as usize {
        let src_row = if top_down { y } else { height as usize - 1 - y };
        let row =
            &pixel_data[src_row * stride..src_row * stride + width as usize * bytes_per_pixel];
        for px in row.chunks_exact(bytes_per_pixel) {
            let value = if bytes_per_pixel == 3 {
                u32::from_le_bytes([px[0], px[1], px[2], 0])
            } else {
                u32::from_le_bytes([px[0], px[1], px[2], px[3]])
            };
            let a = channels[3].extract(value);
            any_alpha |= a != 0;
            pixels.extend_from_slice(&[
                channels[0].extract(value),
                channels[1].extract(value),
                channels[2].extract(value),
                a,
            ]);
        }
    }
    if !any_alpha {
        pixels
            .iter_mut()
            .skip(3)
            .step_by(4)
            .for_each(|a| *a = u8::MAX);
    }
    Some(RgbaImage {
        width,
        height,
        pixels,
    })
}

/// Validates a relative path from the remote file list (`\` or `/` separated) and
/// returns it '/'-separated. Rejects anything that could escape the destination.
pub(crate) fn sanitize_remote_path(name: &str) -> Option<String> {
    let name = name.replace('\\', "/");
    if name.is_empty() || name.starts_with('/') {
        return None;
    }
    let valid = name.split('/').all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part.chars().any(|c| c == ':' || c.is_control())
    });
    valid.then_some(name)
}

/// One entry of a remote file list with its index in the original list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteFileEntry {
    pub index: u32,
    pub file: RemoteFile,
}

/// Parses CLIPRDR_FILELIST (MS-RDPECLIP 2.2.5.2.3). Entries with unsafe names are skipped.
pub(crate) fn decode_file_list(data: &[u8]) -> Option<Vec<RemoteFileEntry>> {
    let count = u32_at(data, 0)? as usize;
    if count > MAX_FILE_LIST_ENTRIES {
        return None;
    }
    let descriptors = data.get(4..4 + count * FILE_DESCRIPTOR_LEN)?;
    let mut entries = Vec::with_capacity(count);
    for (index, desc) in descriptors
        .as_chunks::<FILE_DESCRIPTOR_LEN>()
        .0
        .iter()
        .enumerate()
    {
        let flags = u32_at(desc, 0)?;
        let attributes = u32_at(desc, 36)?;
        let size = (u64::from(u32_at(desc, 64)?) << 32) | u64::from(u32_at(desc, 68)?);
        let units: Vec<u16> = desc[72..72 + FILE_NAME_UNITS * 2]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .take_while(|&unit| unit != 0)
            .collect();
        let Some(path) = String::from_utf16(&units)
            .ok()
            .and_then(|name| sanitize_remote_path(&name))
        else {
            log::warn!("Skipping a remote clipboard file with an unsafe name");
            continue;
        };
        let is_dir = flags & FD_ATTRIBUTES != 0 && attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        entries.push(RemoteFileEntry {
            index: u32::try_from(index).ok()?,
            file: RemoteFile {
                path,
                size: if is_dir || flags & FD_FILESIZE == 0 {
                    0
                } else {
                    size
                },
                is_dir,
            },
        });
    }
    Some(entries)
}

/// A local file or directory offered to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalFileEntry {
    pub path: PathBuf,
    /// Backslash-separated, relative to the parent of the top-level item.
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// The entry is reached through a symbolic link (checked to stay inside its tree).
    pub via_symlink: bool,
}

fn is_valid_windows_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.chars().any(|c| {
            c.is_control() || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
}

fn push_entry(
    out: &mut Vec<LocalFileEntry>,
    path: PathBuf,
    name: String,
    meta: &fs::Metadata,
    via_symlink: bool,
) -> bool {
    if out.len() >= MAX_FILE_LIST_ENTRIES || name.encode_utf16().count() >= FILE_NAME_UNITS {
        return false;
    }
    let is_dir = meta.is_dir();
    out.push(LocalFileEntry {
        path,
        name,
        is_dir,
        size: if is_dir { 0 } else { meta.len() },
        modified: meta.modified().ok(),
        via_symlink,
    });
    true
}

fn walk_directory(
    root: &Path,
    dir: &Path,
    prefix: &str,
    depth: usize,
    out: &mut Vec<LocalFileEntry>,
) {
    if depth > MAX_DIRECTORY_DEPTH {
        return;
    }
    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<_> = read_dir.filter_map(io::Result::ok).collect();
    children.sort_by_key(fs::DirEntry::file_name);
    for child in children {
        let Some(file_name) = child.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !is_valid_windows_name(&file_name) {
            continue;
        }
        let path = child.path();
        let name = format!("{prefix}\\{file_name}");
        let Ok(link_meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if link_meta.file_type().is_symlink() {
            // Only files whose target stays inside the copied tree; never directories.
            let inside = fs::canonicalize(&path).is_ok_and(|target| target.starts_with(root));
            match fs::metadata(&path) {
                Ok(meta) if inside && meta.is_file() && File::open(&path).is_ok() => {
                    if !push_entry(out, path, name, &meta, true) {
                        continue;
                    }
                }
                _ => continue,
            }
        } else if link_meta.is_dir() {
            if fs::read_dir(&path).is_ok()
                && push_entry(out, path.clone(), name.clone(), &link_meta, false)
            {
                walk_directory(root, &path, &name, depth + 1, out);
            }
        } else if link_meta.is_file() && File::open(&path).is_ok() {
            push_entry(out, path, name, &link_meta, false);
        }
    }
}

/// Builds the file list for local `paths`: directories are walked recursively (symbolic
/// links are followed only for top-level items and for files that stay inside the tree);
/// unreadable entries and names Windows cannot represent are skipped.
pub(crate) fn collect_local_files(paths: &[PathBuf]) -> Vec<LocalFileEntry> {
    let mut out = Vec::new();
    for path in paths {
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| is_valid_windows_name(n))
            .map(str::to_owned)
        else {
            continue;
        };
        let Ok(meta) = fs::metadata(path) else {
            continue;
        };
        if meta.is_dir() {
            let Ok(root) = fs::canonicalize(path) else {
                continue;
            };
            if fs::read_dir(&root).is_ok()
                && push_entry(&mut out, root.clone(), name.clone(), &meta, false)
            {
                walk_directory(&root, &root, &name, 1, &mut out);
            }
        } else if meta.is_file() && File::open(path).is_ok() {
            push_entry(&mut out, path.clone(), name, &meta, true);
        }
    }
    out
}

fn filetime(time: Option<SystemTime>) -> u64 {
    let since_epoch = time
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .unwrap_or_default();
    let intervals = since_epoch.as_nanos() / 100;
    u64::try_from(intervals)
        .unwrap_or(u64::MAX)
        .saturating_add(FILETIME_UNIX_EPOCH)
}

/// Encodes CLIPRDR_FILELIST for `entries`.
pub(crate) fn encode_file_list(entries: &[LocalFileEntry]) -> Option<Vec<u8>> {
    let count = u32::try_from(entries.len()).ok()?;
    let mut out = Vec::with_capacity(4 + entries.len() * FILE_DESCRIPTOR_LEN);
    out.extend_from_slice(&count.to_le_bytes());
    for entry in entries {
        let name: Vec<u16> = entry.name.encode_utf16().collect();
        if name.len() >= FILE_NAME_UNITS {
            return None;
        }
        let mut desc = [0u8; FILE_DESCRIPTOR_LEN];
        let flags = FD_ATTRIBUTES | FD_FILESIZE | FD_WRITESTIME | FD_PROGRESSUI;
        let attributes = if entry.is_dir {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            FILE_ATTRIBUTE_NORMAL
        };
        desc[0..4].copy_from_slice(&flags.to_le_bytes());
        desc[36..40].copy_from_slice(&attributes.to_le_bytes());
        desc[56..64].copy_from_slice(&filetime(entry.modified).to_le_bytes());
        let [high, low] = [entry.size >> 32, entry.size & 0xFFFF_FFFF]
            .map(|v| u32::try_from(v).unwrap_or(u32::MAX));
        desc[64..68].copy_from_slice(&high.to_le_bytes());
        desc[68..72].copy_from_slice(&low.to_le_bytes());
        for (dst, unit) in desc[72..].as_chunks_mut::<2>().0.iter_mut().zip(&name) {
            *dst = unit.to_le_bytes();
        }
        out.extend_from_slice(&desc);
    }
    Some(out)
}

/// Answers a CB_FILECONTENTS_REQUEST for a local entry: FILECONTENTS_SIZE writes the size
/// (8 bytes, little endian), FILECONTENTS_RANGE reads up to `buf.len()` bytes at
/// `position`. Returns the number of bytes written.
pub(crate) fn read_local_file(
    entry: &LocalFileEntry,
    flags: u32,
    position: u64,
    buf: &mut [u8],
) -> io::Result<usize> {
    if entry.is_dir {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a file"));
    }
    if !entry.via_symlink && fs::symlink_metadata(&entry.path)?.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "file was replaced by a symbolic link",
        ));
    }
    let mut file = File::open(&entry.path)?;
    if flags & FILECONTENTS_SIZE != 0 {
        let size = file.metadata()?.len().to_le_bytes();
        let dst = buf
            .get_mut(..size.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "buffer too small"))?;
        dst.copy_from_slice(&size);
        return Ok(size.len());
    }
    if flags & FILECONTENTS_RANGE == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unknown request",
        ));
    }
    file.seek(SeekFrom::Start(position))?;
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// Resolves the sanitized relative `rel` ('/'-separated) under the canonical directory
/// `root`, creating missing parent directories (and `rel` itself when `is_dir`). Symbolic
/// links are never followed, so the result always stays inside `root`.
pub(crate) fn safe_target(root: &Path, rel: &str, is_dir: bool) -> io::Result<PathBuf> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "unsafe remote file name");
    let rel = sanitize_remote_path(rel).ok_or_else(invalid)?;
    let parts: Vec<&str> = rel.split('/').collect();
    let mut current = root.to_path_buf();
    for (i, part) in parts.iter().enumerate() {
        let mut components = Path::new(part).components();
        if !matches!(
            (components.next(), components.next()),
            (Some(Component::Normal(_)), None)
        ) {
            return Err(invalid());
        }
        current.push(part);
        let last = i + 1 == parts.len();
        if last && !is_dir {
            break;
        }
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "a file is in the way of a remote directory",
                ))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if let Err(e) = fs::create_dir(&current) {
                    if e.kind() != io::ErrorKind::AlreadyExists {
                        return Err(e);
                    }
                }
                if !fs::symlink_metadata(&current)?.is_dir() {
                    return Err(invalid());
                }
            }
            Err(e) => return Err(e),
        }
    }
    if !current.starts_with(root) || current == root {
        return Err(invalid());
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_round_trip() {
        let text = "héllo 🌍";
        let utf16 = encode_clipboard_text(text, CF_UNICODETEXT).unwrap();
        assert_eq!(&utf16[utf16.len() - 2..], &[0, 0]);
        assert_eq!(decode_utf16le(&utf16), text);
        assert_eq!(
            encode_clipboard_text(text, CF_TEXT).unwrap(),
            b"h\xc3\xa9llo \xf0\x9f\x8c\x8d\0"
        );
        assert_eq!(encode_clipboard_text("", CF_UNICODETEXT), None);
        assert_eq!(encode_clipboard_text(text, 0xC0DE), None);
        assert_eq!(decode_utf16le(&[b'a', 0, b'b']), "a");

        let multiline = "line1\nline2\r\nline3\rline4";
        let norm_utf16 = encode_clipboard_text(multiline, CF_UNICODETEXT).unwrap();
        assert_eq!(
            decode_utf16le(&norm_utf16),
            "line1\r\nline2\r\nline3\r\nline4"
        );
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("wal-clip-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn header(size: u32, width: i32, height: i32, bits: u16, compression: u32) -> Vec<u8> {
        let mut h = vec![0u8; size.max(40) as usize];
        h[0..4].copy_from_slice(&size.to_le_bytes());
        h[4..8].copy_from_slice(&width.to_le_bytes());
        h[8..12].copy_from_slice(&height.to_le_bytes());
        h[12..14].copy_from_slice(&1u16.to_le_bytes());
        h[14..16].copy_from_slice(&bits.to_le_bytes());
        h[16..20].copy_from_slice(&compression.to_le_bytes());
        h
    }

    #[test]
    fn dib_round_trip_keeps_pixels_and_alpha() {
        let image = RgbaImage {
            width: 2,
            height: 2,
            pixels: vec![
                255, 0, 0, 255, 0, 255, 0, 128, //
                0, 0, 255, 0, 10, 20, 30, 40,
            ],
        };
        let dib = encode_dib(&image).unwrap();
        assert_eq!(dib.len(), 40 + 16);
        assert_eq!(u32_at(&dib, 20), Some(16), "biSizeImage");
        assert_eq!(i32_at(&dib, 8), Some(2), "bottom-up");
        // First stored row is the bottom image row, BGRA.
        assert_eq!(&dib[40..44], &[255, 0, 0, 0]);
        assert_eq!(decode_dib(&dib), Some(image));

        assert_eq!(
            encode_dib(&RgbaImage {
                width: 2,
                height: 2,
                pixels: vec![0; 15]
            }),
            None
        );
    }

    #[test]
    fn dib_without_alpha_is_opaque() {
        let image = RgbaImage {
            width: 1,
            height: 2,
            pixels: vec![1, 2, 3, 0, 4, 5, 6, 0],
        };
        let decoded = decode_dib(&encode_dib(&image).unwrap()).unwrap();
        assert_eq!(decoded.pixels, vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn decodes_24bpp_bottom_up_with_row_padding() {
        let mut dib = header(40, 3, 2, 24, BI_RGB);
        // Bottom row: blue, green, red; then top row: white, black, gray. Rows pad to 12.
        dib.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0]);
        dib.extend_from_slice(&[255, 255, 255, 0, 0, 0, 128, 128, 128, 0, 0, 0]);
        let image = decode_dib(&dib).unwrap();
        assert_eq!((image.width, image.height), (3, 2));
        assert_eq!(
            image.pixels,
            vec![
                255, 255, 255, 255, 0, 0, 0, 255, 128, 128, 128, 255, //
                0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255,
            ]
        );
        dib.pop();
        assert_eq!(decode_dib(&dib), None, "truncated pixel data");
    }

    #[test]
    fn decodes_bitfields_top_down_and_v5_alpha() {
        // BITMAPINFOHEADER + masks (RGBX in memory order), top-down.
        let mut dib = header(40, 1, -2, 32, BI_BITFIELDS);
        for mask in [0x0000_00FFu32, 0x0000_FF00, 0x00FF_0000] {
            dib.extend_from_slice(&mask.to_le_bytes());
        }
        dib.extend_from_slice(&[10, 20, 30, 99, 40, 50, 60, 99]);
        let image = decode_dib(&dib).unwrap();
        assert_eq!(image.pixels, vec![10, 20, 30, 255, 40, 50, 60, 255]);

        // V5 header with an alpha mask, bottom-up, masks repeated after the header.
        let mut v5 = header(124, 1, 1, 32, BI_BITFIELDS);
        for (i, mask) in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000]
            .iter()
            .enumerate()
        {
            v5[40 + i * 4..44 + i * 4].copy_from_slice(&mask.to_le_bytes());
        }
        let mut repeated = v5.clone();
        v5.extend_from_slice(&[3, 2, 1, 7]);
        assert_eq!(decode_dib(&v5).unwrap().pixels, vec![1, 2, 3, 7]);
        for mask in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF] {
            repeated.extend_from_slice(&mask.to_le_bytes());
        }
        repeated.extend_from_slice(&[3, 2, 1, 7]);
        assert_eq!(decode_dib(&repeated).unwrap().pixels, vec![1, 2, 3, 7]);
    }

    #[test]
    fn rejects_malformed_dibs() {
        assert_eq!(decode_dib(&[]), None);
        assert_eq!(
            decode_dib(&header(12, 1, 1, 24, BI_RGB)),
            None,
            "core header"
        );
        assert_eq!(
            decode_dib(&header(40, 0, 1, 32, BI_RGB)),
            None,
            "zero width"
        );
        assert_eq!(
            decode_dib(&header(40, 1, 0, 32, BI_RGB)),
            None,
            "zero height"
        );
        assert_eq!(decode_dib(&header(40, 1, i32::MIN, 32, BI_RGB)), None);
        assert_eq!(
            decode_dib(&header(40, 20_000, 1, 32, BI_RGB)),
            None,
            "too wide"
        );
        assert_eq!(decode_dib(&header(40, 1, 1, 8, BI_RGB)), None, "palette");
        assert_eq!(decode_dib(&header(40, 1, 1, 32, 1)), None, "RLE");
        let mut huge = header(40, 16_384, 16_384, 32, BI_RGB);
        huge.extend_from_slice(&[0; 64]);
        assert_eq!(decode_dib(&huge), None, "data shorter than the image");
    }

    #[test]
    fn sanitizes_remote_paths() {
        assert_eq!(
            sanitize_remote_path("a\\b c\\d.txt").as_deref(),
            Some("a/b c/d.txt")
        );
        assert_eq!(sanitize_remote_path("file").as_deref(), Some("file"));
        for bad in [
            "", "\\abs", "/abs", "a\\..\\b", "..", ".", "a\\.\\b", "C:\\x", "C:x", "a\\\\b", "a\\",
            "a\0b", "a\nb", "a\u{1b}b",
        ] {
            assert_eq!(sanitize_remote_path(bad), None, "{bad:?}");
        }
    }

    fn descriptor(name: &str, attributes: u32, size: u64) -> Vec<u8> {
        let mut desc = vec![0u8; FILE_DESCRIPTOR_LEN];
        desc[0..4].copy_from_slice(&(FD_ATTRIBUTES | FD_FILESIZE).to_le_bytes());
        desc[36..40].copy_from_slice(&attributes.to_le_bytes());
        desc[64..68].copy_from_slice(&u32::try_from(size >> 32).unwrap().to_le_bytes());
        desc[68..72].copy_from_slice(&u32::try_from(size & 0xFFFF_FFFF).unwrap().to_le_bytes());
        for (dst, unit) in desc[72..]
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(name.encode_utf16())
        {
            *dst = unit.to_le_bytes();
        }
        desc
    }

    #[test]
    fn decodes_file_lists_and_skips_unsafe_names() {
        let mut data = 4u32.to_le_bytes().to_vec();
        data.extend(descriptor("dir", FILE_ATTRIBUTE_DIRECTORY, 0));
        data.extend(descriptor("..\\evil", FILE_ATTRIBUTE_NORMAL, 1));
        data.extend(descriptor(
            "dir\\big.bin",
            FILE_ATTRIBUTE_NORMAL,
            5 << 32 | 7,
        ));
        data.extend(descriptor("C:\\x", FILE_ATTRIBUTE_NORMAL, 1));
        let entries = decode_file_list(&data).unwrap();
        assert_eq!(
            entries,
            vec![
                RemoteFileEntry {
                    index: 0,
                    file: RemoteFile {
                        path: "dir".into(),
                        size: 0,
                        is_dir: true
                    }
                },
                RemoteFileEntry {
                    index: 2,
                    file: RemoteFile {
                        path: "dir/big.bin".into(),
                        size: 5 << 32 | 7,
                        is_dir: false
                    }
                },
            ]
        );
        data.pop();
        assert_eq!(decode_file_list(&data), None, "truncated");
        assert_eq!(decode_file_list(&u32::MAX.to_le_bytes()), None);
        assert_eq!(decode_file_list(&0u32.to_le_bytes()), Some(vec![]));
    }

    // Symlinks and `:` in file names are Unix-only.
    #[cfg(unix)]
    #[test]
    fn collects_encodes_and_serves_local_files() {
        let tmp = TempDir::new();
        let outside = TempDir::new();
        let top = tmp.0.join("top");
        fs::create_dir_all(top.join("sub")).unwrap();
        fs::write(top.join("b.txt"), b"hello world").unwrap();
        fs::write(top.join("sub").join("c.txt"), b"").unwrap();
        fs::write(top.join("bad:name"), b"x").unwrap();
        fs::write(outside.0.join("secret"), b"s").unwrap();
        std::os::unix::fs::symlink(top.join("b.txt"), top.join("link-in")).unwrap();
        std::os::unix::fs::symlink(outside.0.join("secret"), top.join("link-out")).unwrap();
        std::os::unix::fs::symlink(&outside.0, top.join("link-dir")).unwrap();
        let single = tmp.0.join("single.bin");
        fs::write(&single, [7u8; 10]).unwrap();

        let entries = collect_local_files(&[top.clone(), single.clone(), tmp.0.join("missing")]);
        let names: Vec<(&str, bool)> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.is_dir))
            .collect();
        assert_eq!(
            names,
            vec![
                ("top", true),
                ("top\\b.txt", false),
                ("top\\link-in", false),
                ("top\\sub", true),
                ("top\\sub\\c.txt", false),
                ("single.bin", false),
            ]
        );
        assert_eq!(entries[1].size, 11);

        let encoded = encode_file_list(&entries).unwrap();
        assert_eq!(encoded.len(), 4 + entries.len() * FILE_DESCRIPTOR_LEN);
        assert_eq!(u32_at(&encoded, 4), Some(0x4064), "descriptor flags");
        let decoded = decode_file_list(&encoded).unwrap();
        assert_eq!(decoded.len(), entries.len());
        assert_eq!(decoded[4].file.path, "top/sub/c.txt");
        assert!(decoded[3].file.is_dir);
        assert_eq!(decoded[1].file.size, 11);

        let mut buf = [0u8; 8];
        assert_eq!(
            read_local_file(&entries[1], FILECONTENTS_SIZE, 0, &mut buf).unwrap(),
            8
        );
        assert_eq!(u64::from_le_bytes(buf), 11);
        let mut chunk = [0u8; 5];
        assert_eq!(
            read_local_file(&entries[1], FILECONTENTS_RANGE, 6, &mut chunk).unwrap(),
            5
        );
        assert_eq!(&chunk, b"world");
        assert_eq!(
            read_local_file(&entries[1], FILECONTENTS_RANGE, 100, &mut chunk).unwrap(),
            0
        );
        assert!(read_local_file(&entries[0], FILECONTENTS_SIZE, 0, &mut buf).is_err());
        assert!(read_local_file(&entries[1], FILECONTENTS_SIZE, 0, &mut [0u8; 4]).is_err());
        assert_eq!(
            read_local_file(&entries[2], FILECONTENTS_RANGE, 0, &mut chunk).unwrap(),
            5,
            "in-tree symlink is served"
        );

        fs::remove_file(top.join("b.txt")).unwrap();
        std::os::unix::fs::symlink(outside.0.join("secret"), top.join("b.txt")).unwrap();
        assert!(
            read_local_file(&entries[1], FILECONTENTS_RANGE, 0, &mut chunk).is_err(),
            "file swapped for a symlink"
        );
    }

    #[test]
    fn skips_names_too_long_for_windows() {
        let tmp = TempDir::new();
        let long = tmp.0.join("d".repeat(200));
        fs::create_dir(&long).unwrap();
        fs::write(long.join("f".repeat(100)), b"x").unwrap();
        let entries = collect_local_files(&[long]);
        assert_eq!(entries.len(), 1, "only the directory fits in 259 units");
    }

    #[test]
    fn safe_target_stays_inside_root() {
        let tmp = TempDir::new();
        let outside = TempDir::new();
        let root = &tmp.0;
        assert_eq!(safe_target(root, "a/b", true).unwrap(), root.join("a/b"));
        assert!(root.join("a/b").is_dir());
        assert_eq!(
            safe_target(root, "x/y/file.txt", false).unwrap(),
            root.join("x/y/file.txt")
        );
        assert!(root.join("x/y").is_dir());
        assert!(!root.join("x/y/file.txt").exists());

        for bad in ["", "../x", "a/../../x", "/etc/passwd", "a//b", "."] {
            assert!(safe_target(root, bad, false).is_err(), "{bad:?}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside.0, root.join("link")).unwrap();
            assert!(safe_target(root, "link/file", false).is_err());
            assert!(safe_target(root, "link", true).is_err());
        }
        fs::write(root.join("plain"), b"").unwrap();
        assert!(safe_target(root, "plain/file", false).is_err());
        assert_eq!(fs::read_dir(&outside.0).unwrap().count(), 0);
    }
}
