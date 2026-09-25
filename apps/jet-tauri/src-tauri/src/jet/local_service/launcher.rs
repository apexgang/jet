//! The launcher entry of an AppImage. deb and rpm install their own
//! `.desktop` file; an AppImage (Homebrew moves the cask's to
//! `~/Applications/Jet.AppImage`) has none, so the app writes one for itself
//! at every launch, and rewrites it only when its content differs.
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use super::manager::write_if_changed;

/// The bundle identifier (`tauri.conf.json`), which names the entry and the
/// icon. The Homebrew cask's `zap` removes both files by these names.
pub(crate) const APP_ID: &str = "me.heeka.jet-tauri";
const ICON: &[u8] = include_bytes!("../../../icons/128x128.png");

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refreshed {
    /// Not running as an AppImage.
    NotAppImage,
    /// `$APPIMAGE` is not an absolute path to a regular file, or cannot be
    /// written into a desktop entry safely.
    Invalid,
    Written {
        entry: bool,
        icon: bool,
    },
}

pub(crate) fn entry_path(data_home: &Path) -> PathBuf {
    data_home
        .join("applications")
        .join(format!("{APP_ID}.desktop"))
}

pub(crate) fn icon_path(data_home: &Path) -> PathBuf {
    data_home
        .join("icons/hicolor/128x128/apps")
        .join(format!("{APP_ID}.png"))
}

/// Writes or refreshes the entry and icon for the AppImage at `appimage`.
/// Blocking.
pub(crate) fn refresh(appimage: Option<&OsStr>, data_home: &Path) -> std::io::Result<Refreshed> {
    let Some(appimage) = appimage else {
        return Ok(Refreshed::NotAppImage);
    };
    let Some(entry) = validated(Path::new(appimage)).and_then(|path| desktop_entry(&path)) else {
        return Ok(Refreshed::Invalid);
    };
    let icon = write_if_changed(&icon_path(data_home), ICON, 0o644)?;
    let entry = write_if_changed(&entry_path(data_home), entry.as_bytes(), 0o644)?;
    Ok(Refreshed::Written { entry, icon })
}

/// ASVS 12.3.1: the canonical AppImage path, when it is an absolute
/// regular file.
fn validated(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let canonical = fs::canonicalize(path).ok()?;
    fs::metadata(&canonical)
        .ok()?
        .is_file()
        .then_some(canonical)
}

/// The desktop entry that starts the AppImage at `path`. `TryExec` names
/// the same file, so desktop environments hide the entry once the AppImage
/// is gone: `brew uninstall` without `--zap` leaves the entry behind.
pub(crate) fn desktop_entry(path: &Path) -> Option<String> {
    let path = path.to_str()?;
    let exec = exec_argument(path)?;
    // A plain string value: only the backslash needs its escape (control
    // characters were refused above, and the absolute path never starts
    // with a space).
    let try_exec = path.replace('\\', "\\\\");
    Some(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Jet\n\
         Comment=Start, supervise and return to agent work across computers\n\
         TryExec={try_exec}\n\
         Exec={exec}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=Development;\n"
    ))
}

/// One quoted `Exec` argument (Desktop Entry Specification, "The Exec
/// key"): inside double quotes, `"`, `` ` ``, `$` and `\` are escaped with a
/// backslash; the string-value escape then doubles every backslash; `%`
/// becomes `%%` so it is never a field code. Control characters cannot be
/// represented and are refused.
pub(crate) fn exec_argument(path: &str) -> Option<String> {
    if path.chars().any(char::is_control) {
        return None;
    }
    let mut quoted = String::with_capacity(path.len() + 2);
    quoted.push('"');
    for character in path.chars() {
        match character {
            // `\"` after quoting, `\\"` after the string escape.
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(character);
            }
            '\\' => quoted.push_str("\\\\\\\\"),
            '%' => quoted.push_str("%%"),
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    Some(quoted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_arguments_are_escaped_per_the_specification() {
        let cases = [
            (
                "/home/u/Applications/Jet.AppImage",
                r#""/home/u/Applications/Jet.AppImage""#,
            ),
            (
                "/home/u/My Apps/Jet.AppImage",
                r#""/home/u/My Apps/Jet.AppImage""#,
            ),
            ("/a/$HOME/x", r#""/a/\\$HOME/x""#),
            ("/a/\"q\"/x", r#""/a/\\"q\\"/x""#),
            ("/a/`id`/x", r#""/a/\\`id\\`/x""#),
            ("/a/b\\c", r#""/a/b\\\\c""#),
            ("/a/100%/x", r#""/a/100%%/x""#),
        ];
        for (path, expected) in cases {
            assert_eq!(exec_argument(path).as_deref(), Some(expected), "{path}");
        }
        assert_eq!(exec_argument("/a/line\nbreak"), None);
        assert_eq!(exec_argument("/a/tab\there"), None);
    }

    #[test]
    fn try_exec_names_the_appimage_as_a_string_value() {
        let entry = desktop_entry(Path::new("/a/My Apps/b\\c/Jet.AppImage")).unwrap();
        assert!(
            entry.contains("\nTryExec=/a/My Apps/b\\\\c/Jet.AppImage\n"),
            "{entry}"
        );
        assert_eq!(desktop_entry(Path::new("/a/line\nbreak")), None);
    }

    #[test]
    fn the_identifier_matches_the_bundle() {
        let configuration: serde_json::Value =
            serde_json::from_str(include_str!("../../../tauri.conf.json")).unwrap();
        assert_eq!(configuration["identifier"], APP_ID);
    }

    #[test]
    fn an_appimage_gets_an_entry_and_icon_written_once() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let appimage = root.path().join("Applications/Jet.AppImage");
        fs::create_dir_all(appimage.parent().unwrap()).unwrap();
        fs::write(&appimage, b"AI").unwrap();

        assert_eq!(refresh(None, &data).unwrap(), Refreshed::NotAppImage);
        assert_eq!(
            refresh(Some(OsStr::new("relative/Jet.AppImage")), &data).unwrap(),
            Refreshed::Invalid
        );
        assert_eq!(
            refresh(Some(root.path().join("missing").as_os_str()), &data).unwrap(),
            Refreshed::Invalid
        );
        assert_eq!(
            refresh(Some(root.path().as_os_str()), &data).unwrap(),
            Refreshed::Invalid,
            "a directory"
        );
        assert!(!data.exists());

        assert_eq!(
            refresh(Some(appimage.as_os_str()), &data).unwrap(),
            Refreshed::Written {
                entry: true,
                icon: true
            }
        );
        let entry = fs::read_to_string(entry_path(&data)).unwrap();
        let canonical = fs::canonicalize(&appimage).unwrap();
        assert!(entry.contains(&format!("Exec=\"{}\"\n", canonical.display())));
        assert!(entry.contains(&format!("\nTryExec={}\n", canonical.display())));
        assert!(entry.contains("Icon=me.heeka.jet-tauri\n"));
        assert_eq!(fs::read(icon_path(&data)).unwrap(), ICON);

        assert_eq!(
            refresh(Some(appimage.as_os_str()), &data).unwrap(),
            Refreshed::Written {
                entry: false,
                icon: false
            }
        );
    }
}
