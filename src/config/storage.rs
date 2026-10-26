//! Private on-disk storage: everything under the config directory is owner-only.

use std::fs;
use std::io;
use std::path::Path;

/// Creates `dir` (and parents) with mode 0700 on Unix.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        // Tighten directories created by older versions.
        let perms = fs::metadata(dir)?.permissions();
        if perms.mode() & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

/// Atomically writes `contents` to `path`, readable by the owner only.
pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let unique = uuid::Uuid::new_v4();
    let tmp = path.with_extension(format!("tmp.{unique}"));
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        io::Write::write_all(&mut file, contents)?;
        file.sync_all()?;
    }
    if let Err(e) = fs::rename(&tmp, path) {
        // Fallback for Windows where destination might require explicit removal
        let _ = fs::remove_file(path);
        if let Err(retry_err) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(if e.kind() == io::ErrorKind::PermissionDenied {
                retry_err
            } else {
                e
            });
        }
    }
    Ok(())
}

/// Removes `path`, treating "already gone" as success.
pub fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn writes_owner_only_files() {
        let dir = std::env::temp_dir().join(format!("wal-storage-{}", uuid::Uuid::new_v4()));
        let file = dir.join("nested").join("secret.json");
        write_private(&file, b"{}").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"{}");
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(file.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
