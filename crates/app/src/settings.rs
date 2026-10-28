use radial_domain::{Config, ConfigError};
use thiserror::Error;

use crate::ports::ConfigRepository;

#[derive(Debug, Error)]
pub enum SaveError<E> {
    #[error(transparent)]
    Invalid(#[from] ConfigError),
    #[error("could not save the settings: {0}")]
    Store(E),
}

/// Validates `config` and saves it. The caller then sends [`crate::Event::ConfigChanged`].
pub fn save_config<R: ConfigRepository>(
    repo: &R,
    config: &Config,
) -> Result<(), SaveError<R::Error>> {
    config.validate()?;
    repo.save(config).map_err(SaveError::Store)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    struct MemoryRepository(RefCell<Option<Config>>);

    impl ConfigRepository for MemoryRepository {
        type Error = std::io::Error;
        fn load(&self) -> Result<Config, std::io::Error> {
            Ok(self.0.borrow().clone().unwrap_or_default())
        }
        fn save(&self, config: &Config) -> Result<(), std::io::Error> {
            *self.0.borrow_mut() = Some(config.clone());
            Ok(())
        }
    }

    #[test]
    fn saves_a_valid_config() {
        let repo = MemoryRepository::default();
        save_config(&repo, &Config::default()).unwrap();
        assert_eq!(*repo.0.borrow(), Some(Config::default()));
    }

    #[test]
    fn refuses_an_invalid_config() {
        let repo = MemoryRepository::default();
        let mut config = Config::default();
        config.snippets[0].label.clear();
        let err = save_config(&repo, &config).unwrap_err();
        assert!(matches!(
            err,
            SaveError::Invalid(ConfigError::EmptyLabel { index: 0 })
        ));
        assert!(repo.0.borrow().is_none());
    }
}
