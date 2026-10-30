//! The owner's choices, kept in a small JSON file so they survive a restart.

use crate::decision::Mode;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub mode: Mode,
    pub battery_sends_to_github: bool,
    /// Hard CPU cap for the runner and everything a job starts, in percent; 100 = none.
    pub cpu_limit_percent: u8,
    /// Folder of the official GitHub runner (the one with `run.cmd`).
    pub runner_dir: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            mode: Mode::Automatic,
            battery_sends_to_github: true,
            cpu_limit_percent: 100,
            runner_dir: PathBuf::new(),
        }
    }
}

/// A missing file is the first run, not an error: it gives the defaults.
/// A file that exists but does not parse is an error, so a typo never
/// silently turns the runner back on.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(ConfigError::Parse),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(ConfigError::Io(e)),
    }
}

/// Writes to a temporary file and renames it, so a crash never leaves half a file.
pub fn save(path: &Path, config: &Config) -> Result<(), ConfigError> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(ConfigError::Io)?;
    }
    let text = serde_json::to_string_pretty(config).map_err(ConfigError::Parse)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(ConfigError::Io)?;
    fs::rename(&tmp, path).map_err(ConfigError::Io)
}

#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Parse(serde_json::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "config file: {e}"),
            ConfigError::Parse(e) => write!(f, "config file is not valid JSON: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scr-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("config.json")
    }

    #[test]
    fn missing_file_gives_defaults() {
        let path = temp_path("missing");
        assert_eq!(load(&path).unwrap(), Config::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let path = temp_path("roundtrip");
        let config = Config {
            mode: Mode::Disabled,
            battery_sends_to_github: false,
            cpu_limit_percent: 50,
            runner_dir: PathBuf::from(r"C:\runner"),
        };
        save(&path, &config).unwrap();
        assert_eq!(load(&path).unwrap(), config);
    }

    #[test]
    fn broken_file_is_an_error_not_defaults() {
        let path = temp_path("broken");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ not json").unwrap();
        assert!(matches!(load(&path), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn empty_file_is_an_error_not_defaults() {
        let path = temp_path("empty");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "").unwrap();
        assert!(matches!(load(&path), Err(ConfigError::Parse(_))));
    }
}
