//! `planes.json`: the remote Planes this computer knows, persisted only
//! after their identity was proven. It holds no key material, no cursors and
//! no pairing codes, and is written atomically with mode 0600 (the
//! `identity.rs` pattern). Anything invalid resets the registry once, keeps
//! the unreadable file as `planes.json.invalid`, and tells the user.
//!
//! Two cases keep the file untouched instead (D8): a registry written by a
//! newer Jet (an app rollback must not drop every remote Plane), and a read
//! that failed for another reason, such as a permission error, which may
//! pass. Both load as an empty, read-only registry with a notice: nothing
//! is written back until a later launch can read the file.
use std::{
    fs::{self, OpenOptions},
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use jet_client::SshEndpoint;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::jet::{errors::PublicError, keystore::Credential};

const FILE: &str = "planes.json";
const INVALID: &str = "planes.json.invalid";
const VERSION: u32 = 1;
const MAXIMUM_BYTES: u64 = 16 * 1024;
pub(crate) const MAXIMUM_REMOTE_PLANES: usize = 16;
/// Longest SSH address accepted from the webview.
const MAXIMUM_DESTINATION: usize = 255;
pub(crate) const REGISTRY_RESET: &str = "registry_reset";
/// A newer Jet wrote the registry; it is kept as it is.
pub(crate) const REGISTRY_NEWER: &str = "registry_newer";
/// The registry could not be read this time; it is kept as it is.
pub(crate) const REGISTRY_UNREADABLE: &str = "registry_unreadable";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredPlane {
    pub(crate) id: Uuid,
    pub(crate) destination: String,
    pub(crate) plane_identity: Uuid,
    pub(crate) credential: Credential,
    pub(crate) added_at_unix_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Stored {
    pub(crate) local_identity: Option<Uuid>,
    pub(crate) planes: Vec<StoredPlane>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileV1 {
    version: u32,
    local_identity: Option<String>,
    planes: Vec<FilePlane>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FilePlane {
    id: String,
    destination: String,
    plane_identity: String,
    credential: String,
    added_at_unix_ms: i64,
}

/// An SSH address split for duplicate checks: the account name compares
/// exactly (it is case-sensitive), the host ASCII case-insensitively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DestinationKey {
    user: Option<String>,
    host: String,
}

impl DestinationKey {
    pub(crate) fn of(destination: &str) -> Self {
        match destination.split_once('@') {
            Some((user, host)) => Self {
                user: Some(user.to_owned()),
                host: host.to_ascii_lowercase(),
            },
            None => Self {
                user: None,
                host: destination.to_ascii_lowercase(),
            },
        }
    }
}

/// Validates a webview-supplied SSH address as data, never as options.
pub(crate) fn validate_destination(value: &str) -> Result<(String, SshEndpoint), PublicError> {
    let trimmed = value.trim();
    let invalid = || {
        PublicError::invalid_input(
            "plane.destination_invalid",
            "Enter an SSH address such as user@host, or a host alias from your SSH config.",
        )
    };
    if trimmed.is_empty() || trimmed.len() > MAXIMUM_DESTINATION {
        return Err(invalid());
    }
    let endpoint = SshEndpoint::new(trimmed).map_err(|_| invalid())?;
    Ok((trimmed.to_owned(), endpoint))
}

/// Whether `bytes` is a registry of a version newer than this app reads.
fn newer_version(bytes: &[u8]) -> bool {
    #[derive(Deserialize)]
    struct Versioned {
        version: u32,
    }
    serde_json::from_slice::<Versioned>(bytes).is_ok_and(|file| file.version > VERSION)
}

fn canonical_uuid(value: &str) -> Option<Uuid> {
    let id = Uuid::parse_str(value).ok()?;
    (id.to_string() == value).then_some(id)
}

fn parse(bytes: &[u8]) -> Option<Stored> {
    let file: FileV1 = serde_json::from_slice(bytes).ok()?;
    if file.version != VERSION || file.planes.len() > MAXIMUM_REMOTE_PLANES {
        return None;
    }
    let local_identity = match file.local_identity {
        Some(value) => Some(canonical_uuid(&value)?),
        None => None,
    };
    let mut planes: Vec<StoredPlane> = Vec::with_capacity(file.planes.len());
    for plane in file.planes {
        let (destination, _) = validate_destination(&plane.destination).ok()?;
        if destination != plane.destination {
            return None;
        }
        let stored = StoredPlane {
            id: canonical_uuid(&plane.id)?,
            plane_identity: canonical_uuid(&plane.plane_identity)?,
            credential: match plane.credential.as_str() {
                "durable" => Credential::Durable,
                "session" => Credential::Session,
                _ => return None,
            },
            added_at_unix_ms: plane.added_at_unix_ms,
            destination,
        };
        let key = DestinationKey::of(&stored.destination);
        if planes.iter().any(|other| {
            other.id == stored.id
                || other.plane_identity == stored.plane_identity
                || DestinationKey::of(&other.destination) == key
        }) {
            return None;
        }
        planes.push(stored);
    }
    Some(Stored {
        local_identity,
        planes,
    })
}

fn encode(stored: &Stored) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec_pretty(&FileV1 {
        version: VERSION,
        local_identity: stored.local_identity.map(|id| id.to_string()),
        planes: stored
            .planes
            .iter()
            .map(|plane| FilePlane {
                id: plane.id.to_string(),
                destination: plane.destination.clone(),
                plane_identity: plane.plane_identity.to_string(),
                credential: plane.credential.name().to_owned(),
                added_at_unix_ms: plane.added_at_unix_ms,
            })
            .collect(),
    })
}

pub(crate) struct RegistryFile {
    directory: PathBuf,
    /// Set by a load that had to leave the file as it is: saving would
    /// replace Planes this launch could not read.
    read_only: bool,
}

impl RegistryFile {
    pub(crate) fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_owned(),
            read_only: false,
        }
    }

    /// Whether saves are refused for this launch (`load`).
    pub(crate) fn read_only(&self) -> bool {
        self.read_only
    }

    fn path(&self) -> PathBuf {
        self.directory.join(FILE)
    }

    /// The stored registry, and `registry_reset` when the file had to be set
    /// aside. A missing file is an empty registry without a notice. A newer
    /// registry or a failed read is left in place and makes this file
    /// read-only for the launch (`registry_newer`, `registry_unreadable`).
    pub(crate) fn load(&mut self) -> (Stored, Option<&'static str>) {
        let path = self.path();
        let read = (|| -> io::Result<Option<Vec<u8>>> {
            let file = match fs::File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
            let mut bytes = Vec::new();
            file.take(MAXIMUM_BYTES + 1).read_to_end(&mut bytes)?;
            Ok(Some(bytes))
        })();
        match read {
            Ok(None) => (Stored::default(), None),
            Ok(Some(bytes)) if bytes.len() as u64 <= MAXIMUM_BYTES => match parse(&bytes) {
                Some(stored) => (stored, None),
                None if newer_version(&bytes) => self.keep(REGISTRY_NEWER),
                None => self.reset(),
            },
            Ok(Some(_)) => self.reset(),
            // A directory in its place can be set aside without losing data.
            Err(error) if error.kind() == ErrorKind::IsADirectory => self.reset(),
            Err(_) => self.keep(REGISTRY_UNREADABLE),
        }
    }

    /// Leaves the file as it is and refuses saves for this launch.
    fn keep(&mut self, notice: &'static str) -> (Stored, Option<&'static str>) {
        self.read_only = true;
        (Stored::default(), Some(notice))
    }

    /// Sets the unreadable file aside (one slot, overwritten) and starts empty.
    fn reset(&self) -> (Stored, Option<&'static str>) {
        let _ = fs::rename(self.path(), self.directory.join(INVALID));
        (Stored::default(), Some(REGISTRY_RESET))
    }

    /// Writes atomically: a new 0600 temporary file, fsync, then rename.
    /// Refused while the file is read-only for this launch.
    pub(crate) fn save(&self, stored: &Stored) -> io::Result<()> {
        if self.read_only {
            return Err(io::Error::from(ErrorKind::ReadOnlyFilesystem));
        }
        let bytes = encode(stored).map_err(|_| io::Error::from(ErrorKind::InvalidData))?;
        if bytes.len() as u64 > MAXIMUM_BYTES {
            return Err(io::Error::from(ErrorKind::InvalidData));
        }
        fs::create_dir_all(&self.directory)?;
        let temporary = self.directory.join(format!(
            ".{FILE}.{}.{}.tmp",
            std::process::id(),
            Uuid::new_v4()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let written = (|| {
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, self.path())
        })();
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        written?;
        #[cfg(unix)]
        fs::set_permissions(self.path(), fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(id: u128, destination: &str, identity: u128, credential: Credential) -> StoredPlane {
        StoredPlane {
            id: Uuid::from_u128(id),
            destination: destination.into(),
            plane_identity: Uuid::from_u128(identity),
            credential,
            added_at_unix_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn the_registry_round_trips_with_credentials_and_local_identity() {
        let directory = tempfile::tempdir().unwrap();
        let mut file = RegistryFile::new(directory.path());
        assert_eq!(file.load(), (Stored::default(), None));
        let stored = Stored {
            local_identity: Some(Uuid::from_u128(1)),
            planes: vec![
                plane(2, "alice@build-box", 20, Credential::Durable),
                plane(3, "ci", 30, Credential::Session),
            ],
        };
        file.save(&stored).unwrap();
        assert_eq!(file.load(), (stored, None));
        let text = fs::read_to_string(directory.path().join(FILE)).unwrap();
        assert!(text.contains(r#""credential": "session""#));
        assert!(text.contains(r#""localIdentity": "00000000-0000-0000-0000-000000000001""#));
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(directory.path().join(FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let leftovers: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn an_invalid_or_oversized_file_is_set_aside_with_a_notice() {
        let invalid_files = [
            r#"{"version":0,"localIdentity":null,"planes":[]}"#.to_owned(),
            r#"{"version":1,"localIdentity":null,"planes":[],"extra":1}"#.to_owned(),
            r#"{"version":1,"localIdentity":"nope","planes":[]}"#.to_owned(),
            r#"{"version":1,"localIdentity":null,"planes":[{"id":"00000000-0000-0000-0000-000000000002","destination":"-oProxyCommand=x","planeIdentity":"00000000-0000-0000-0000-000000000003","credential":"durable","addedAtUnixMs":1}]}"#.to_owned(),
            r#"{"version":1,"localIdentity":null,"planes":[{"id":"00000000-0000-0000-0000-000000000002","destination":"host","planeIdentity":"00000000-0000-0000-0000-000000000003","credential":"file","addedAtUnixMs":1}]}"#.to_owned(),
            "not json".to_owned(),
            format!(r#"{{"version":1,"localIdentity":null,"planes":[],"pad":"{}"}}"#, "x".repeat(17_000)),
        ];
        for contents in invalid_files {
            let directory = tempfile::tempdir().unwrap();
            fs::write(directory.path().join(FILE), &contents).unwrap();
            let mut file = RegistryFile::new(directory.path());
            assert_eq!(
                file.load(),
                (Stored::default(), Some(REGISTRY_RESET)),
                "{contents:.80}"
            );
            assert!(!directory.path().join(FILE).exists());
            assert_eq!(
                fs::read_to_string(directory.path().join(INVALID)).unwrap(),
                contents
            );
            // Only once: the next start finds no file and no notice.
            assert_eq!(file.load(), (Stored::default(), None));
        }
    }

    /// D8: a registry from a newer Jet is not set aside. It stays in place,
    /// byte for byte, and nothing is written over it this launch.
    #[test]
    fn a_newer_registry_is_kept_read_only() {
        let directory = tempfile::tempdir().unwrap();
        let newer = r#"{"version":2,"localIdentity":null,"planes":[],"added":"later"}"#;
        fs::write(directory.path().join(FILE), newer).unwrap();
        let mut file = RegistryFile::new(directory.path());
        assert_eq!(file.load(), (Stored::default(), Some(REGISTRY_NEWER)));
        assert!(file.read_only());
        assert!(!directory.path().join(INVALID).exists());
        let error = file.save(&Stored::default()).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ReadOnlyFilesystem);
        assert_eq!(
            fs::read_to_string(directory.path().join(FILE)).unwrap(),
            newer
        );
        // Every launch says so again until a Jet that reads it runs.
        let mut again = RegistryFile::new(directory.path());
        assert_eq!(again.load().1, Some(REGISTRY_NEWER));
    }

    /// D8: a read that fails (here, permission denied) keeps the file as it
    /// is instead of resetting the registry; a later launch reads it.
    #[cfg(unix)]
    #[test]
    fn a_failed_read_keeps_the_registry_for_a_later_launch() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let stored = Stored {
            local_identity: None,
            planes: vec![plane(2, "alice@build-box", 20, Credential::Durable)],
        };
        RegistryFile::new(directory.path()).save(&stored).unwrap();
        let path = directory.path().join(FILE);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::File::open(&path).is_ok() {
            // Running as root: permissions do not apply.
            return;
        }
        let mut file = RegistryFile::new(directory.path());
        assert_eq!(file.load(), (Stored::default(), Some(REGISTRY_UNREADABLE)));
        assert!(file.save(&Stored::default()).is_err());
        assert!(!directory.path().join(INVALID).exists());

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(RegistryFile::new(directory.path()).load(), (stored, None));
    }

    #[test]
    fn at_most_sixteen_remote_planes_are_stored() {
        let directory = tempfile::tempdir().unwrap();
        let mut file = RegistryFile::new(directory.path());
        let planes: Vec<_> = (0..17u128)
            .map(|index| {
                plane(
                    100 + index,
                    &format!("host-{index}"),
                    200 + index,
                    Credential::Durable,
                )
            })
            .collect();
        let contents = String::from_utf8(
            encode(&Stored {
                local_identity: None,
                planes,
            })
            .unwrap(),
        )
        .unwrap();
        fs::write(directory.path().join(FILE), contents).unwrap();
        assert_eq!(file.load().1, Some(REGISTRY_RESET));
    }

    #[test]
    fn destinations_compare_user_exactly_and_host_case_insensitively() {
        assert_ne!(
            DestinationKey::of("Alice@host"),
            DestinationKey::of("alice@host")
        );
        assert_eq!(
            DestinationKey::of("alice@HOST"),
            DestinationKey::of("alice@host")
        );
        assert_eq!(
            DestinationKey::of("Build-Box"),
            DestinationKey::of("build-box")
        );
        assert_ne!(DestinationKey::of("alice@host"), DestinationKey::of("host"));
    }

    #[test]
    fn destinations_are_validated_as_data() {
        assert_eq!(
            validate_destination("  alice@host ").unwrap().0,
            "alice@host"
        );
        for invalid in [
            "",
            "   ",
            "-oProxyCommand=evil",
            "a@b@c",
            "host name",
            "host;rm",
            &"h".repeat(256),
        ] {
            assert_eq!(
                validate_destination(invalid).err().unwrap().code,
                "plane.destination_invalid",
                "{invalid:.20}"
            );
        }
    }
}
