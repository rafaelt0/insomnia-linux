//! Owner-only JSON files. Only state that outlives the process lives here:
//! the session deadline and the sinks a lid close muted. Sleep inhibition and
//! the power-saver hold are released by logind and power-profiles-daemon when
//! the process exits, so they have no journal.

use crate::model::{Config, Session};
use serde::{Serialize, de::DeserializeOwned};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Journal {
    /// PulseAudio sink names this app muted and has not unmuted yet.
    pub muted_sinks: Vec<String>,
}

pub enum Loaded<T> {
    Missing,
    Ok(T),
    /// The file did not parse. It was renamed to the path given.
    MovedAside(PathBuf),
}

pub struct Store {
    state_dir: PathBuf,
    config_dir: PathBuf,
}

impl Store {
    /// `INSOMNIA_HOME` puts everything under one directory, for tests.
    pub fn from_env() -> Self {
        let env_dir = |var: &str, fallback: &str| {
            std::env::var_os(var)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback))
        };
        match std::env::var_os("INSOMNIA_HOME") {
            Some(home) => Store { state_dir: home.clone().into(), config_dir: home.into() },
            None => Store {
                state_dir: env_dir("XDG_STATE_HOME", ".local/state").join("insomnia"),
                config_dir: env_dir("XDG_CONFIG_HOME", ".config").join("insomnia"),
            },
        }
    }

    pub fn config_path(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    fn session_path(&self) -> PathBuf {
        self.state_dir.join("session.json")
    }

    fn journal_path(&self) -> PathBuf {
        self.state_dir.join("journal.json")
    }

    pub fn load_session(&self) -> Loaded<Session> {
        load(&self.session_path())
    }

    pub fn save_session(&self, session: &Session) -> std::io::Result<()> {
        save(&self.session_path(), session)
    }

    pub fn delete_session(&self) -> std::io::Result<()> {
        match fs::remove_file(self.session_path()) {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    pub fn load_journal(&self) -> Loaded<Journal> {
        load(&self.journal_path())
    }

    pub fn save_journal(&self, journal: &Journal) -> std::io::Result<()> {
        save(&self.journal_path(), journal)
    }

    /// Reads config.json, repairing and writing it back when needed. A missing
    /// file is created with the defaults.
    pub fn load_config(&self) -> (Config, Vec<String>) {
        let path = self.config_path();
        let (raw, mut notes) = match load::<Config>(&path) {
            Loaded::Ok(c) => (c, vec![]),
            Loaded::Missing => (Config::default(), vec!["created config.json with the defaults".into()]),
            Loaded::MovedAside(to) => (Config::default(), vec![format!("config.json did not parse and was moved to {}", to.display())]),
        };
        let (config, fixes) = raw.normalized();
        notes.extend(fixes);
        if !notes.is_empty()
            && let Err(e) = self.save_config(&config)
        {
            notes.push(format!("could not write config.json: {e}"));
        }
        (config, notes)
    }

    pub fn save_config(&self, config: &Config) -> std::io::Result<()> {
        save(&self.config_path(), config)
    }
}

fn load<T: DeserializeOwned>(path: &Path) -> Loaded<T> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == ErrorKind::NotFound => return Loaded::Missing,
        Err(_) => return move_aside(path),
    };
    match serde_json::from_slice(&bytes) {
        Ok(v) => Loaded::Ok(v),
        Err(_) => move_aside(path),
    }
}

/// Never deletes an unreadable file: renames it beside itself so it can be inspected.
fn move_aside<T>(path: &Path) -> Loaded<T> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut to = path.as_os_str().to_owned();
    to.push(format!(".unreadable-{stamp}"));
    let to = PathBuf::from(to);
    match fs::rename(path, &to) {
        Ok(()) => Loaded::MovedAside(to),
        Err(_) => Loaded::MovedAside(path.to_owned()),
    }
}

fn save<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let dir = path.parent().expect("store paths have a parent");
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    let tmp = path.with_extension("json.tmp");
    let mut file = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    file.write_all(&serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?)?;
    file.sync_all()?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("insomnia-store-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&dir);
        (Store { state_dir: dir.join("state"), config_dir: dir.join("config") }, dir)
    }

    #[test]
    fn session_round_trips_owner_only() {
        let (store, dir) = temp_store();
        let s = Session { started_at: 1, ends_at: 2 };
        store.save_session(&s).unwrap();
        assert!(matches!(store.load_session(), Loaded::Ok(read) if read == s));
        let mode = |p: PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dir.join("state/session.json")), 0o600);
        assert_eq!(mode(dir.join("state")), 0o700);
        store.delete_session().unwrap();
        store.delete_session().unwrap();
        assert!(matches!(store.load_session(), Loaded::Missing));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unreadable_session_is_moved_aside_not_deleted() {
        let (store, dir) = temp_store();
        fs::create_dir_all(dir.join("state")).unwrap();
        fs::write(dir.join("state/session.json"), b"{not json").unwrap();
        let Loaded::MovedAside(to) = store.load_session() else { panic!("expected a moved file") };
        assert_eq!(fs::read(&to).unwrap(), b"{not json");
        assert!(matches!(store.load_session(), Loaded::Missing));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn config_is_created_then_repaired_and_written_back() {
        let (store, dir) = temp_store();
        let (c, notes) = store.load_config();
        assert_eq!((c, notes.len()), (Config::default(), 1));
        fs::write(store.config_path(), br#"{"lowPowerFloor": 5, "endFloor": 10}"#).unwrap();
        let (c, notes) = store.load_config();
        assert_eq!((c.low_power_floor, notes.len()), (15, 1));
        let (_, notes) = store.load_config();
        assert!(notes.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
}
