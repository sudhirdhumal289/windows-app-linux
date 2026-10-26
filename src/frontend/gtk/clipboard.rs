//! Rich clipboard between GTK and a session: images and files, both directions.

use crate::rdp::{LocalClipboard, RemoteFileFetcher, RgbaImage};
use gtk4::gdk::{self, prelude::*, subclass::prelude::*};
use gtk4::{gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Largest local image offered to a session (in pixels; 8K UHD).
const MAX_IMAGE_PIXELS: usize = 7680 * 4320;

const URI_LIST: &str = "text/uri-list";
const GNOME_COPIED_FILES: &str = "x-special/gnome-copied-files";

/// Clipboard image from the session.
pub fn texture_from_image(image: &RgbaImage) -> Option<gdk::Texture> {
    let stride = usize::try_from(image.width).ok()?.checked_mul(4)?;
    let expected = stride.checked_mul(usize::try_from(image.height).ok()?)?;
    if image.width == 0 || image.height == 0 || image.pixels.len() != expected {
        return None;
    }
    let bytes = glib::Bytes::from(&image.pixels);
    let texture = gdk::MemoryTexture::new(
        i32::try_from(image.width).ok()?,
        i32::try_from(image.height).ok()?,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        stride,
    );
    Some(texture.upcast())
}

/// Local clipboard image for the session (tightly packed RGBA).
fn image_from_texture(texture: &gdk::Texture) -> Option<RgbaImage> {
    let width = usize::try_from(texture.width()).ok()?;
    let height = usize::try_from(texture.height()).ok()?;
    if width == 0 || height == 0 || width.checked_mul(height)? > MAX_IMAGE_PIXELS {
        return None;
    }
    let mut downloader = gdk::TextureDownloader::new(texture);
    downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    let row = width * 4;
    let mut pixels = Vec::with_capacity(row * height);
    for y in 0..height {
        let start = y.checked_mul(stride)?;
        pixels.extend_from_slice(bytes.get(start..start.checked_add(row)?)?);
    }
    Some(RgbaImage {
        width: u32::try_from(width).ok()?,
        height: u32::try_from(height).ok()?,
        pixels,
    })
}

/// Reads the local clipboard as the richest content the session can take: files, then
/// an image, then text. `None` when it holds nothing usable.
pub async fn read_local(clipboard: &gdk::Clipboard) -> Option<LocalClipboard> {
    let formats = clipboard.formats();
    if formats.contains_type(gdk::FileList::static_type()) {
        if let Ok(value) = clipboard
            .read_value_future(gdk::FileList::static_type(), glib::Priority::DEFAULT)
            .await
        {
            let paths: Vec<PathBuf> = value
                .get::<gdk::FileList>()
                .map(|list| list.files().iter().filter_map(gio::File::path).collect())
                .unwrap_or_default();
            if !paths.is_empty() {
                return Some(LocalClipboard::Files(paths));
            }
        }
    }
    if formats.contains_type(gdk::Texture::static_type()) {
        if let Ok(Some(texture)) = clipboard.read_texture_future().await {
            if let Some(image) = image_from_texture(&texture) {
                return Some(LocalClipboard::Image(image));
            }
        }
    }
    match clipboard.read_text_future().await {
        Ok(Some(text)) => Some(LocalClipboard::Text(text.to_string())),
        _ => None,
    }
}

/// Whether the clipboard holds files offered by the session `owner` itself (which must
/// not be sent back to it).
pub fn is_own_files(clipboard: &gdk::Clipboard, owner: &str) -> bool {
    clipboard
        .content()
        .and_downcast::<RemoteFilesProvider>()
        .is_some_and(|provider| provider.imp().owner.borrow().as_str() == owner)
}

glib::wrapper! {
    /// Offers the session's copied files to local applications. They are downloaded
    /// (once) when an application pastes them.
    pub struct RemoteFilesProvider(ObjectSubclass<imp::RemoteFilesProvider>)
        @extends gdk::ContentProvider;
}

impl RemoteFilesProvider {
    /// `owner` identifies the session; files are downloaded into `dest`.
    pub fn new(owner: &str, fetcher: RemoteFileFetcher, dest: PathBuf) -> Self {
        let provider: Self = glib::Object::new();
        let imp = provider.imp();
        *imp.owner.borrow_mut() = owner.to_string();
        *imp.fetcher.borrow_mut() = Some(fetcher);
        *imp.dest.borrow_mut() = dest;
        provider
    }
}

/// Builds the bytes for a file `mime_type` from downloaded `paths`.
fn encode_paths(mime_type: &str, paths: &[PathBuf]) -> Option<Vec<u8>> {
    let uris: Vec<String> = paths
        .iter()
        .filter_map(|p| glib::filename_to_uri(p, None).ok())
        .map(|uri| uri.to_string())
        .collect();
    if uris.is_empty() {
        return None;
    }
    match mime_type {
        URI_LIST => Some(
            uris.iter()
                .map(|uri| format!("{uri}\r\n"))
                .collect::<String>()
                .into_bytes(),
        ),
        GNOME_COPIED_FILES => Some(format!("copy\n{}", uris.join("\n")).into_bytes()),
        _ => None,
    }
}

/// Download state shared by concurrent paste requests.
#[derive(Default)]
enum Download {
    #[default]
    NotStarted,
    Running(Vec<tokio::sync::oneshot::Sender<Result<Rc<[PathBuf]>, String>>>),
    Done(Rc<[PathBuf]>),
}

mod imp {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;

    #[derive(Default)]
    pub struct RemoteFilesProvider {
        pub(super) owner: RefCell<String>,
        pub(super) fetcher: RefCell<Option<RemoteFileFetcher>>,
        pub(super) dest: RefCell<PathBuf>,
        pub(super) download: RefCell<Download>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RemoteFilesProvider {
        const NAME: &'static str = "WalRemoteFilesProvider";
        type Type = super::RemoteFilesProvider;
        type ParentType = gdk::ContentProvider;
    }

    impl ObjectImpl for RemoteFilesProvider {}

    impl ContentProviderImpl for RemoteFilesProvider {
        fn formats(&self) -> gdk::ContentFormats {
            gdk::ContentFormatsBuilder::new()
                .add_mime_type(URI_LIST)
                .add_mime_type(GNOME_COPIED_FILES)
                .build()
        }

        fn write_mime_type_future(
            &self,
            mime_type: &str,
            stream: &gio::OutputStream,
            io_priority: glib::Priority,
        ) -> Pin<Box<dyn Future<Output = Result<(), glib::Error>> + 'static>> {
            let provider = self.obj().clone();
            let mime_type = mime_type.to_string();
            let stream = stream.clone();
            Box::pin(async move {
                let paths = provider
                    .imp()
                    .downloaded()
                    .await
                    .map_err(|e| glib::Error::new(gio::IOErrorEnum::Failed, &e))?;
                let bytes = encode_paths(&mime_type, &paths).ok_or_else(|| {
                    glib::Error::new(gio::IOErrorEnum::NotSupported, "Unsupported format")
                })?;
                stream
                    .write_all_future(bytes, io_priority)
                    .await
                    .map_err(|(_, e)| e)?;
                Ok(())
            })
        }
    }

    impl RemoteFilesProvider {
        /// Paths of the downloaded files, downloading them on first use.
        async fn downloaded(&self) -> Result<Rc<[PathBuf]>, String> {
            let receiver = {
                let mut download = self.download.borrow_mut();
                match &mut *download {
                    Download::Done(paths) => return Ok(Rc::clone(paths)),
                    Download::Running(waiters) => {
                        let (tx, rx) = tokio::sync::oneshot::channel();
                        waiters.push(tx);
                        Some(rx)
                    }
                    Download::NotStarted => {
                        *download = Download::Running(Vec::new());
                        None
                    }
                }
            };
            if let Some(receiver) = receiver {
                return receiver
                    .await
                    .unwrap_or_else(|_| Err("The download was cancelled".to_string()));
            }

            let fetcher = self.fetcher.borrow().clone();
            let dest = self.dest.borrow().clone();
            let result = match fetcher {
                Some(fetcher) => gio::spawn_blocking(move || fetch(&fetcher, &dest))
                    .await
                    .unwrap_or_else(|_| Err("The download failed".to_string())),
                None => Err("The files are no longer available".to_string()),
            };
            let waiters = match std::mem::take(&mut *self.download.borrow_mut()) {
                Download::Running(waiters) => waiters,
                _ => Vec::new(),
            };
            let result = result.map(Rc::<[PathBuf]>::from);
            if let Ok(paths) = &result {
                *self.download.borrow_mut() = Download::Done(Rc::clone(paths));
            }
            for waiter in waiters {
                let _ = waiter.send(result.clone());
            }
            result
        }
    }

    fn fetch(fetcher: &RemoteFileFetcher, dest: &Path) -> Result<Vec<PathBuf>, String> {
        fetcher.fetch(dest).map_err(|e| {
            log::warn!("Downloading the remote clipboard files failed: {e}");
            format!("Could not copy the files from the remote PC: {e}")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_uri_list_and_gnome_formats() {
        let paths = vec![PathBuf::from("/tmp/a b.txt"), PathBuf::from("/tmp/c")];
        let list = String::from_utf8(encode_paths(URI_LIST, &paths).unwrap()).unwrap();
        assert_eq!(list, "file:///tmp/a%20b.txt\r\nfile:///tmp/c\r\n");
        let gnome = String::from_utf8(encode_paths(GNOME_COPIED_FILES, &paths).unwrap()).unwrap();
        assert_eq!(gnome, "copy\nfile:///tmp/a%20b.txt\nfile:///tmp/c");
        assert!(encode_paths("text/plain", &paths).is_none());
        assert!(encode_paths(URI_LIST, &[]).is_none());
    }

    #[test]
    fn rejects_inconsistent_images() {
        let image = RgbaImage {
            width: 2,
            height: 2,
            pixels: vec![0; 15],
        };
        assert!(texture_from_image(&image).is_none());
    }
}
