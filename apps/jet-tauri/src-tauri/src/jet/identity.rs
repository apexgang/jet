use std::{
    fs::{self, OpenOptions},
    io::{self, ErrorKind, Write},
    path::Path,
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use uuid::Uuid;

const CLIENT_ID_FILE: &str = "client-id";

pub(crate) fn load_or_create(app_data_directory: &Path) -> io::Result<Uuid> {
    // ASVS 3.1.1 and 3.5.1: native identity metadata is owner-only and never
    // stored in browser storage. The client UUID is an identifier, not a
    // credential: `list_planes` shows it so the user can tell which paired
    // client is this computer.
    fs::create_dir_all(app_data_directory)?;
    #[cfg(unix)]
    fs::set_permissions(app_data_directory, fs::Permissions::from_mode(0o700))?;

    let path = app_data_directory.join(CLIENT_ID_FILE);
    match fs::read_to_string(&path) {
        Ok(value) => parse(&value),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let id = Uuid::new_v4();
            write_atomically(app_data_directory, &path, id)?;
            Ok(id)
        }
        Err(error) => Err(error),
    }
}

fn parse(value: &str) -> io::Result<Uuid> {
    if value.len() > 40 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "Jet client identity is oversized",
        ));
    }
    Uuid::parse_str(value.trim())
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "Jet client identity is invalid"))
}

fn write_atomically(directory: &Path, destination: &Path, id: Uuid) -> io::Result<()> {
    let temporary = directory.join(format!(".{CLIENT_ID_FILE}.{}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    writeln!(file, "{id}")?;
    file.sync_all()?;
    fs::rename(&temporary, destination)?;
    #[cfg(unix)]
    fs::set_permissions(destination, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::load_or_create;

    #[test]
    fn client_identity_is_stable_without_entering_web_storage() {
        let directory = tempfile::tempdir().unwrap();
        let first = load_or_create(directory.path()).unwrap();
        let second = load_or_create(directory.path()).unwrap();
        assert_eq!(first, second);
    }
}
