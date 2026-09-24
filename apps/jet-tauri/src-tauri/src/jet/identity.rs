//! This installation's client identity (`client-id`): a UUID, not a
//! credential. Loading it never fails, so a damaged app data directory can
//! never keep the window from opening (Wave 4, D5). An unreadable file is
//! set aside as `client-id.invalid`; the identity is then recovered from the
//! keyring item that holds this computer's pairing key when exactly one
//! exists, because a new identity orphans every pairing, and created afresh
//! otherwise. Either way the Planes panel says what happened.

use std::{fs, io, path::Path};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use uuid::Uuid;

use super::local_store;

const CLIENT_ID_FILE: &str = "client-id";
const INVALID_FILE: &str = "client-id.invalid";
/// A UUID, a newline and some slack for whitespace.
const MAX_CLIENT_ID_BYTES: u64 = 64;

/// What happened to the stored identity at launch, for the Planes notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdentityNotice {
    /// Unreadable, set aside, and recovered from the keyring.
    Recovered,
    /// Unreadable and set aside; a new identity replaces it.
    Replaced,
    /// It could not be saved: the next launch will not find it.
    Unsaved,
}

impl IdentityNotice {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Recovered => "identity_recovered",
            Self::Replaced => "identity_replaced",
            Self::Unsaved => "identity_unsaved",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LoadedIdentity {
    pub(crate) client_id: Uuid,
    pub(crate) notice: Option<IdentityNotice>,
}

/// The stored identity, or a recovered or new one when it is missing or
/// unreadable. `recover` is asked only for an unreadable file.
pub(crate) fn load_or_create(
    app_data_directory: &Path,
    recover: impl FnOnce() -> Option<Uuid>,
) -> LoadedIdentity {
    // ASVS 3.1.1 and 3.5.1: native identity metadata is owner-only and never
    // stored in browser storage. The client UUID is an identifier, not a
    // credential: `list_planes` shows it so the user can tell which paired
    // client is this computer.
    if fs::create_dir_all(app_data_directory).is_err() {
        return LoadedIdentity {
            client_id: Uuid::new_v4(),
            notice: Some(IdentityNotice::Unsaved),
        };
    }
    // Best effort: every file inside is written 0600 on its own.
    #[cfg(unix)]
    let _ = fs::set_permissions(app_data_directory, fs::Permissions::from_mode(0o700));

    let path = app_data_directory.join(CLIENT_ID_FILE);
    let read = local_store::read_bounded(&path, MAX_CLIENT_ID_BYTES).map(|bytes| bytes.map(parse));
    let (client_id, mut notice) = match read {
        Ok(Some(Some(id))) => {
            return LoadedIdentity {
                client_id: id,
                notice: None,
            }
        }
        Ok(None) => (Uuid::new_v4(), None),
        // Empty, garbage, oversized, a directory, or unreadable.
        Ok(Some(None)) | Err(_) => {
            set_aside(app_data_directory);
            match recover() {
                Some(id) => (id, Some(IdentityNotice::Recovered)),
                None => (Uuid::new_v4(), Some(IdentityNotice::Replaced)),
            }
        }
    };
    if save(app_data_directory, client_id).is_err() {
        // The next launch will not find this identity, recovered or new.
        notice = Some(IdentityNotice::Unsaved);
    }
    LoadedIdentity { client_id, notice }
}

fn parse(bytes: Vec<u8>) -> Option<Uuid> {
    let value = String::from_utf8(bytes).ok()?;
    let id = Uuid::parse_str(value.trim()).ok()?;
    (!id.is_nil()).then_some(id)
}

/// Keeps the unreadable file (or directory) for inspection, replacing an
/// older one. If that fails, the save below replaces a file in place.
fn set_aside(directory: &Path) {
    let invalid = directory.join(INVALID_FILE);
    if invalid.is_dir() {
        let _ = fs::remove_dir_all(&invalid);
    }
    let _ = fs::rename(directory.join(CLIENT_ID_FILE), invalid);
}

/// An owner-only, fsynced, atomic replace with a unique temporary name, so
/// a temporary file left behind by an earlier launch never blocks it.
fn save(directory: &Path, id: Uuid) -> io::Result<()> {
    local_store::write_private_atomically(directory, CLIENT_ID_FILE, format!("{id}\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use uuid::Uuid;

    use super::{load_or_create, IdentityNotice, LoadedIdentity, CLIENT_ID_FILE, INVALID_FILE};

    #[test]
    fn client_identity_is_stable_without_entering_web_storage() {
        let directory = tempfile::tempdir().unwrap();
        let first = load_or_create(directory.path(), || panic!("nothing to recover"));
        assert_eq!(first.notice, None);
        let second = load_or_create(directory.path(), || panic!("nothing to recover"));
        assert_eq!(first, second);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(directory.path().join(CLIENT_ID_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    /// D5: an unreadable identity never fails the launch. It is set aside,
    /// recovered from the keyring when possible, and saved again.
    #[test]
    fn an_unreadable_identity_is_set_aside_and_recovered() {
        let directory = tempfile::tempdir().unwrap();
        let recovered = Uuid::from_u128(0x1d);
        fs::write(directory.path().join(CLIENT_ID_FILE), "not a uuid").unwrap();
        let loaded = load_or_create(directory.path(), || Some(recovered));
        assert_eq!(
            loaded,
            LoadedIdentity {
                client_id: recovered,
                notice: Some(IdentityNotice::Recovered),
            }
        );
        assert_eq!(
            fs::read_to_string(directory.path().join(INVALID_FILE)).unwrap(),
            "not a uuid"
        );
        let again = load_or_create(directory.path(), || panic!("readable now"));
        assert_eq!(again.client_id, recovered);
        assert_eq!(again.notice, None);
    }

    /// D5: without a keyring answer a new identity replaces it, and says so.
    #[test]
    fn an_unrecoverable_identity_is_replaced_with_a_notice() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(CLIENT_ID_FILE), "").unwrap();
        let loaded = load_or_create(directory.path(), || None);
        assert_eq!(loaded.notice, Some(IdentityNotice::Replaced));
        assert_eq!(
            load_or_create(directory.path(), || None).client_id,
            loaded.client_id
        );
    }

    /// D5: a temporary file an earlier launch left behind (the old name was
    /// the PID with `create_new`) no longer blocks saving the identity.
    #[test]
    fn a_stale_temporary_file_does_not_block_the_save() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory
                .path()
                .join(format!(".{CLIENT_ID_FILE}.{}.tmp", std::process::id())),
            "stale",
        )
        .unwrap();
        let loaded = load_or_create(directory.path(), || None);
        assert_eq!(loaded.notice, None);
        assert_eq!(
            load_or_create(directory.path(), || None).client_id,
            loaded.client_id
        );
    }

    /// D5: an app data path that cannot be a directory still yields an
    /// identity for this launch.
    #[test]
    fn an_unusable_app_data_directory_gives_a_session_identity() {
        let directory = tempfile::tempdir().unwrap();
        let blocked = directory.path().join("app-data");
        fs::write(&blocked, "a file where the directory should be").unwrap();
        let loaded = load_or_create(&blocked, || panic!("nothing was read"));
        assert_eq!(loaded.notice, Some(IdentityNotice::Unsaved));
    }
}
