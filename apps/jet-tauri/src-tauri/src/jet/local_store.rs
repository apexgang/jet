//! Client-local files in the app data directory: a bounded read that never
//! loads more than its limit, and an owner-only, fsynced, atomic replace
//! (the `identity.rs` pattern). Blocking: async callers use
//! `spawn_blocking`.

use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use uuid::Uuid;

/// The file's bytes, `None` when it does not exist, or `InvalidData` when it
/// is larger than `max`. The size is checked before reading, and the read
/// itself stops after `max + 1` bytes in case the file grew in between.
pub(crate) fn read_bounded(path: &Path, max: u64) -> io::Result<Option<Vec<u8>>> {
    let file = match open_regular(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if file.metadata()?.len() > max {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    read_limited(file, max).map(Some)
}

/// Opens `path` for reading only when it is a regular file. A FIFO, socket
/// or device in its place would block the read (and launch) or never end,
/// so it is `InvalidData`, as damaged content is; a directory is
/// `IsADirectory`. The open itself never waits: on Linux it is
/// non-blocking, which changes nothing for a regular file.
pub(crate) fn open_regular(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    options.custom_flags(O_NONBLOCK);
    let file = options.open(path)?;
    let kind = file.metadata()?.file_type();
    if kind.is_dir() {
        Err(io::Error::from(io::ErrorKind::IsADirectory))
    } else if kind.is_file() {
        Ok(file)
    } else {
        Err(io::Error::from(io::ErrorKind::InvalidData))
    }
}

/// `O_NONBLOCK` on every Linux architecture this app builds for.
#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0o4000;

fn read_limited(reader: impl Read, max: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    Ok(bytes)
}

/// Replaces `directory/name` with `bytes`: a fresh temporary file (0600),
/// written and synced, then renamed over the destination. A reader sees
/// the old file or the new one, never a partial one. The temporary file is
/// removed on any failure.
pub(crate) fn write_private_atomically(
    directory: &Path,
    name: &str,
    bytes: &[u8],
) -> io::Result<()> {
    let destination = directory.join(name);
    let temporary = directory.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let result = options
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temporary, &destination));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        return result;
    }
    #[cfg(unix)]
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_files(directory: &Path) -> usize {
        fs::read_dir(directory)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count()
    }

    #[test]
    fn atomic_write_replaces_without_partial_file_and_sets_0600() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        fs::write(&path, b"old").unwrap();
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        write_private_atomically(directory.path(), "state.json", b"{\"new\":true}").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"new\":true}");
        assert_eq!(temporary_files(directory.path()), 0);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        // A destination that can't be replaced leaves no temporary file behind.
        fs::create_dir(directory.path().join("taken")).unwrap();
        fs::write(directory.path().join("taken").join("inner"), b"x").unwrap();
        assert!(write_private_atomically(directory.path(), "taken", b"new").is_err());
        assert_eq!(temporary_files(directory.path()), 0);
    }

    #[test]
    fn bounded_read_rejects_by_metadata_without_reading() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.json");
        fs::write(&path, vec![b' '; 2048]).unwrap();
        let error = read_bounded(&path, 1024).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        fs::write(&path, vec![b' '; 1024]).unwrap();
        assert_eq!(read_bounded(&path, 1024).unwrap().unwrap().len(), 1024);
    }

    #[test]
    fn bounded_read_rejects_growth_past_limit() {
        // The file grew after its size was checked: the read still stops.
        let grown = io::repeat(b'x').take(10_000);
        let error = read_limited(grown, 1024).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(read_limited(&b"12345"[..], 5).unwrap(), b"12345");
    }

    /// Finding 8: a FIFO in a file's place (`client-id`, `planes.json`)
    /// used to block the read, and launch with it, until a writer came.
    #[cfg(unix)]
    #[test]
    fn a_fifo_is_damaged_content_and_never_blocks() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client-id");
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let (done, finished) = std::sync::mpsc::channel();
        let reader = path.clone();
        std::thread::spawn(move || {
            let _ = done.send(read_bounded(&reader, 64).map_err(|error| error.kind()));
        });
        let read = finished
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("reading a FIFO must not block");
        assert_eq!(read, Err(io::ErrorKind::InvalidData));
        assert_eq!(
            open_regular(directory.path()).unwrap_err().kind(),
            io::ErrorKind::IsADirectory
        );
    }

    #[test]
    fn bounded_read_missing_is_none() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            read_bounded(&directory.path().join("missing.json"), 1024).unwrap(),
            None
        );
    }
}
