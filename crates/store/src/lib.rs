//! Settings kept as TOML at `%APPDATA%\radial-paste\config.toml`.
#![forbid(unsafe_code)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use radial_app::ports::ConfigRepository;
use radial_domain::{Config, ConfigError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("no roaming app data folder was found")]
    NoConfigDir,
    #[error("could not read {}: {source}", .path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("could not write {}: {source}", .path.display())]
    Write { path: PathBuf, source: io::Error },
    #[error("{} is not valid settings: {source}", .path.display())]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("{}: {source}", .path.display())]
    Invalid { path: PathBuf, source: ConfigError },
    #[error("could not write the settings as TOML: {0}")]
    Serialize(#[from] toml::ser::Error),
}

pub struct TomlConfigRepository {
    path: PathBuf,
}

impl TomlConfigRepository {
    /// `%APPDATA%\radial-paste\config.toml`.
    pub fn at_default_location() -> Result<Self, StoreError> {
        let dirs = directories::BaseDirs::new().ok_or(StoreError::NoConfigDir)?;
        Ok(Self::at(
            dirs.config_dir().join("radial-paste").join("config.toml"),
        ))
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn write_error(&self, source: io::Error) -> StoreError {
        StoreError::Write {
            path: self.path.clone(),
            source,
        }
    }
}

impl ConfigRepository for TomlConfigRepository {
    type Error = StoreError;

    /// A missing file gives the default settings, without writing them.
    fn load(&self) -> Result<Config, StoreError> {
        let path = || self.path.clone();
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(source) => {
                return Err(StoreError::Read {
                    path: path(),
                    source,
                });
            }
        };
        let config: Config = toml::from_str(&text).map_err(|source| StoreError::Parse {
            path: path(),
            source,
        })?;
        config.validate().map_err(|source| StoreError::Invalid {
            path: path(),
            source,
        })?;
        Ok(config)
    }

    /// Writes a temporary file beside the real one and renames it over, so a crash never
    /// leaves half a file.
    fn save(&self, config: &Config) -> Result<(), StoreError> {
        let text = toml::to_string_pretty(config)?;
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|e| self.write_error(e))?;
        }
        let temporary = self.path.with_extension("toml.tmp");
        fs::write(&temporary, text).map_err(|e| self.write_error(e))?;
        fs::rename(&temporary, &self.path).map_err(|e| self.write_error(e))
    }
}

#[cfg(test)]
mod tests {
    use radial_domain::Snippet;

    use super::*;

    fn repo(dir: &tempfile::TempDir) -> TomlConfigRepository {
        TomlConfigRepository::at(dir.path().join("nested").join("config.toml"))
    }

    #[test]
    fn missing_file_loads_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        assert_eq!(repo.load().unwrap(), Config::default());
        assert!(!repo.path().exists());
    }

    #[test]
    fn round_trips_multi_line_text_and_the_hotkey() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        let config = Config {
            hotkey: "Ctrl+Alt+F13".parse().unwrap(),
            snippets: vec![Snippet::new("Sig", "Best,\n\"Quoted\" name\n")],
            ..Config::default()
        };
        repo.save(&config).unwrap();
        assert_eq!(repo.load().unwrap(), config);

        let text = fs::read_to_string(repo.path()).unwrap();
        assert!(text.contains("hotkey = \"Ctrl+Alt+F13\""), "{text}");
        assert!(text.contains("[[snippet]]"), "{text}");
    }

    #[test]
    fn missing_fields_take_their_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        fs::create_dir_all(repo.path().parent().unwrap()).unwrap();
        fs::write(
            repo.path(),
            "[[snippet]]\nlabel = \"Hi\"\ntext = \"Hello\"\n",
        )
        .unwrap();
        let config = repo.load().unwrap();
        assert_eq!(config.hotkey, Config::default().hotkey);
        assert_eq!(config.snippets, [Snippet::new("Hi", "Hello")]);
    }

    #[test]
    fn reports_bad_toml_bad_hotkeys_and_invalid_settings() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        fs::create_dir_all(repo.path().parent().unwrap()).unwrap();

        fs::write(repo.path(), "version = ").unwrap();
        assert!(matches!(repo.load(), Err(StoreError::Parse { .. })));

        fs::write(repo.path(), "hotkey = \"Win+Q\"").unwrap();
        assert!(matches!(repo.load(), Err(StoreError::Parse { .. })));

        fs::write(repo.path(), "version = 7").unwrap();
        assert!(matches!(
            repo.load(),
            Err(StoreError::Invalid {
                source: ConfigError::UnsupportedVersion(7),
                ..
            })
        ));
    }
}
