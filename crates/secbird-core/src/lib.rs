//! Lightweight control of Razer peripherals over USB HID.
//!
//! ```no_run
//! let api = hidapi::HidApi::new()?;
//! let db = secbird_core::Db::builtin()?;
//! for dev in secbird_core::discover(&api, &db) {
//!     println!("{} firmware {:?}", dev.name(), dev.firmware()?);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod actions;
pub mod buttons;
pub mod commands;
#[cfg(windows)]
pub mod debloat;
pub mod db;
pub mod device;
pub mod effects;
pub mod error;
pub mod firmware;
pub mod hid_descriptor;
pub mod features;
pub mod headset;
pub mod laptop;
pub mod log;
pub mod macros;
pub mod net;
pub mod profile;
pub mod proto;
pub mod report;
pub mod scroll;
pub mod sim;
pub mod studio;
pub mod transport;
pub mod update;

pub use db::{Db, DeviceSpec, Kind, Lighting, Matrix};
pub use device::{discover, Device};
pub use error::{Error, Result};
pub use features::{Dimming, DpiStages, GameMode, Rgb, SleepTimer};
pub use profile::{Profile, Store, Zone};
pub use scroll::{Assist, ScrollMode, ScrollSettings, ScrollWheel};
pub use transport::Transport;
pub use hidapi;

/// The product name people see.
pub const APP_NAME: &str = "Occam";
/// The name of the repository, the executables and the settings folder.
pub const FILE_NAME: &str = "occam-rzr";

/// `%APPDATA%\occam-rzr`: profiles.json and the user's devices.json.
#[cfg(windows)]
pub fn config_dir() -> std::path::PathBuf {
    config_root().join(FILE_NAME)
}

/// `$XDG_CONFIG_HOME/occam-rzr`, usually `~/.config/occam-rzr`.
#[cfg(not(windows))]
pub fn config_dir() -> std::path::PathBuf {
    config_root().join(FILE_NAME)
}

/// Where the settings folder lives: `%APPDATA%`, or `$XDG_CONFIG_HOME` / `~/.config`.
fn config_root() -> std::path::PathBuf {
    #[cfg(windows)]
    let root = std::env::var_os("APPDATA").map(std::path::PathBuf::from);
    #[cfg(not(windows))]
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")));
    root.unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// The settings folder this project had as Secretarybird (`%APPDATA%\Secretarybird`,
/// `~/.config/secretarybird`).
fn old_config_dir() -> std::path::PathBuf {
    config_root().join(if cfg!(windows) { "Secretarybird" } else { "secretarybird" })
}

/// First start after the rename: copy the old Secretarybird settings folder into the new
/// one, so profiles, tray settings and the log carry over. The old folder stays as it was,
/// to go back to an older version. Does nothing once the new folder exists. Returns whether
/// it copied.
pub fn migrate_config() -> std::io::Result<bool> {
    migrate_config_from(&old_config_dir(), &config_dir())
}

fn migrate_config_from(old: &std::path::Path, new: &std::path::Path) -> std::io::Result<bool> {
    if new.exists() || !old.is_dir() {
        return Ok(false);
    }
    // Into a temporary folder first, then one rename: a copy cut short never leaves a
    // half-filled settings folder that would stop the next start from copying.
    let partial = new.with_extension("partial");
    let _ = std::fs::remove_dir_all(&partial);
    copy_dir(old, &partial)?;
    for (from, to) in [("secbird.log", "occam-rzr.log"), ("secbird.log.old", "occam-rzr.log.old")] {
        if partial.join(from).exists() {
            std::fs::rename(partial.join(from), partial.join(to))?;
        }
    }
    std::fs::rename(&partial, new)?;
    Ok(true)
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn old_settings_are_copied_once_and_left_in_place() {
        let base = std::env::temp_dir().join(format!("occam-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (old, new) = (base.join("Secretarybird"), base.join("occam-rzr"));
        std::fs::create_dir_all(old.join("sub")).unwrap();
        std::fs::write(old.join("profiles.json"), "{}").unwrap();
        std::fs::write(old.join("secbird.log"), "line").unwrap();
        std::fs::write(old.join("sub").join("x"), "y").unwrap();

        assert!(super::migrate_config_from(&old, &new).unwrap());
        assert_eq!(std::fs::read_to_string(new.join("profiles.json")).unwrap(), "{}");
        assert_eq!(std::fs::read_to_string(new.join("occam-rzr.log")).unwrap(), "line");
        assert!(new.join("sub").join("x").exists());
        assert!(old.join("profiles.json").exists(), "the old folder stays");

        // Once the new folder exists, nothing is copied over it again.
        std::fs::write(new.join("profiles.json"), "{\"new\":1}").unwrap();
        assert!(!super::migrate_config_from(&old, &new).unwrap());
        assert_eq!(std::fs::read_to_string(new.join("profiles.json")).unwrap(), "{\"new\":1}");
        // No old folder: nothing to do.
        assert!(!super::migrate_config_from(&base.join("none"), &base.join("other")).unwrap());
        let _ = std::fs::remove_dir_all(&base);
    }
}