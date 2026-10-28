use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{HotkeyBinding, Layout};

/// The settings format this build reads and writes.
pub const CONFIG_VERSION: u32 = 1;
/// One ring of wedges. More will come as a second ring or pages, not as nested menus.
pub const MAX_SNIPPETS: usize = 8;
/// Longer labels do not fit on a wedge; the full text goes in [`Snippet::text`].
pub const MAX_LABEL_CHARS: usize = 24;

/// One entry of the menu: a short label for its wedge and the text it pastes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    pub label: String,
    pub text: String,
}

impl Snippet {
    pub fn new(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            text: text.into(),
        }
    }
}

/// Everything the user can change. Missing fields take their defaults.
// Plain values come before `layout` and `snippets` so that TOML can write them before its tables.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub hotkey: HotkeyBinding,
    /// How long to wait, once a snippet is chosen, for the hotkey's modifiers to be let go
    /// before pasting anyway.
    pub modifier_release_timeout_ms: u64,
    /// Start with Windows.
    pub autostart: bool,
    pub layout: Layout,
    /// In wedge order: the first at 12 o'clock, the rest clockwise.
    #[serde(rename = "snippet")]
    pub snippets: Vec<Snippet>,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("settings version {0} is not supported (this build reads version {CONFIG_VERSION})")]
    UnsupportedVersion(u32),
    #[error("{0} snippets is too many, the menu holds {MAX_SNIPPETS}")]
    TooManySnippets(usize),
    #[error("snippet {} has no label", .index + 1)]
    EmptyLabel { index: usize },
    #[error("the label of snippet {} is longer than {MAX_LABEL_CHARS} characters", .index + 1)]
    LabelTooLong { index: usize },
    #[error("snippet {} has no text", .index + 1)]
    EmptyText { index: usize },
    #[error("the menu's radius must be larger than its dead zone")]
    InvalidLayout,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            hotkey: HotkeyBinding::default(),
            modifier_release_timeout_ms: 300,
            autostart: false,
            layout: Layout::default(),
            snippets: vec![
                Snippet::new("Thanks", "Thank you!"),
                Snippet::new("On it", "I'm on it and will get back to you shortly."),
                Snippet::new("Regards", "Kind regards,\n"),
                Snippet::new("Email", "name@example.com"),
            ],
        }
    }
}

impl Config {
    pub fn modifier_release_timeout(&self) -> Duration {
        Duration::from_millis(self.modifier_release_timeout_ms)
    }

    /// The hotkey is valid by construction; this checks the rest.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion(self.version));
        }
        if self.snippets.len() > MAX_SNIPPETS {
            return Err(ConfigError::TooManySnippets(self.snippets.len()));
        }
        for (index, snippet) in self.snippets.iter().enumerate() {
            let label = snippet.label.trim();
            if label.is_empty() {
                return Err(ConfigError::EmptyLabel { index });
            }
            if label.chars().count() > MAX_LABEL_CHARS {
                return Err(ConfigError::LabelTooLong { index });
            }
            if snippet.text.is_empty() {
                return Err(ConfigError::EmptyText { index });
            }
        }
        if !self.layout.is_valid() {
            return Err(ConfigError::InvalidLayout);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_snippets(snippets: Vec<Snippet>) -> Config {
        Config {
            snippets,
            ..Config::default()
        }
    }

    #[test]
    fn default_is_valid() {
        assert_eq!(Config::default().validate(), Ok(()));
    }

    #[test]
    fn holds_at_most_one_ring() {
        let full = with_snippets(vec![Snippet::new("a", "a"); MAX_SNIPPETS]);
        assert_eq!(full.validate(), Ok(()));
        let over = with_snippets(vec![Snippet::new("a", "a"); MAX_SNIPPETS + 1]);
        assert_eq!(over.validate(), Err(ConfigError::TooManySnippets(9)));
    }

    #[test]
    fn labels_must_be_short_and_not_blank() {
        let blank = with_snippets(vec![Snippet::new("ok", "x"), Snippet::new("  ", "x")]);
        assert_eq!(blank.validate(), Err(ConfigError::EmptyLabel { index: 1 }));
        assert_eq!(
            ConfigError::EmptyLabel { index: 1 }.to_string(),
            "snippet 2 has no label"
        );

        let fits = with_snippets(vec![Snippet::new("é".repeat(MAX_LABEL_CHARS), "x")]);
        assert_eq!(fits.validate(), Ok(()));
        let long = with_snippets(vec![Snippet::new("é".repeat(MAX_LABEL_CHARS + 1), "x")]);
        assert_eq!(long.validate(), Err(ConfigError::LabelTooLong { index: 0 }));
    }

    #[test]
    fn text_must_not_be_empty() {
        let empty = with_snippets(vec![Snippet::new("a", "")]);
        assert_eq!(empty.validate(), Err(ConfigError::EmptyText { index: 0 }));
    }

    #[test]
    fn rejects_other_versions_and_bad_layouts() {
        let newer = Config {
            version: CONFIG_VERSION + 1,
            ..Config::default()
        };
        assert_eq!(newer.validate(), Err(ConfigError::UnsupportedVersion(2)));

        let mut squashed = Config::default();
        squashed.layout.radius = squashed.layout.dead_zone;
        assert_eq!(squashed.validate(), Err(ConfigError::InvalidLayout));
    }
}
