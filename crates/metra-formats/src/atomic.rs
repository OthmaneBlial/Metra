use std::io;
use std::path::Path;

/// Replace an existing destination with a fully written temporary file.
///
/// Unix `rename` already replaces a destination atomically. Windows needs the
/// explicit `MoveFileExW` replace flag; using the same helper from every writer
/// keeps the safety contract independent of the host platform.
pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };

        let source = wide_path(source);
        let destination = wide_path(destination);
        // SAFETY: both vectors are NUL-terminated UTF-16 paths owned for the
        // duration of the call, as required by MoveFileExW.
        let replaced = unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if replaced == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    #[cfg(not(windows))]
    {
        std::fs::rename(source, destination)
    }
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::atomic_replace;

    #[test]
    fn replaces_existing_destination() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-atomic-{unique}"));
        fs::create_dir(&directory).expect("temporary directory should be created");
        let source = directory.join("source");
        let destination = directory.join("destination");
        fs::write(&source, b"new").expect("source should be written");
        fs::write(&destination, b"old").expect("destination should be written");

        atomic_replace(&source, &destination).expect("destination should be replaced");

        assert_eq!(
            fs::read(&destination).expect("destination should remain"),
            b"new"
        );
        assert!(!source.exists(), "temporary source should be consumed");
        fs::remove_file(&destination).expect("destination should be removed");
        fs::remove_dir(&directory).expect("temporary directory should be removed");
    }
}
