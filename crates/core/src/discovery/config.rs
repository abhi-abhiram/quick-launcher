//! The user's `config.toml` (*Plugin discovery* in `docs/protocol.md`).

use std::collections::BTreeMap;

use serde::Deserialize;

/// User overrides. Unknown keys are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Config {
    /// Plugin ids to skip.
    #[serde(default)]
    pub disabled: Vec<String>,
    /// Per-plugin overrides, by id.
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginConfig>,
}

/// `[plugins.<id>]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct PluginConfig {
    /// Replaces the manifest's prefixes.
    pub prefixes: Option<Vec<String>>,
}

impl Config {
    /// Parses `config.toml`'s text. The error is one line, for the log.
    pub fn parse(text: &str) -> Result<Config, String> {
        toml::from_str(text).map_err(|error| super::manifest::one_line(&error))
    }

    pub fn is_disabled(&self, id: &str) -> bool {
        self.disabled.iter().any(|disabled| disabled == id)
    }

    /// The prefixes the user set for `id`, if any.
    pub fn prefixes(&self, id: &str) -> Option<&[String]> {
        self.plugins.get(id)?.prefixes.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_example_parses() {
        let config = Config::parse(
            r#"
            disabled = ["terminal"]          # plugin ids to skip

            [plugins.pi]
            prefixes = ["p:", "pi:"]         # replaces the manifest's prefixes
            "#,
        )
        .unwrap();
        assert!(config.is_disabled("terminal"));
        assert!(!config.is_disabled("pi"));
        assert_eq!(
            config.prefixes("pi"),
            Some(&["p:".to_owned(), "pi:".to_owned()][..])
        );
        assert_eq!(config.prefixes("terminal"), None);
    }

    #[test]
    fn an_empty_file_is_the_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let config = Config::parse(
            r#"
            theme = "dark"
            [plugins.pi]
            colour = "red"
            "#,
        )
        .unwrap();
        assert_eq!(config.prefixes("pi"), None);
    }

    #[test]
    fn a_wrong_type_is_an_error_on_one_line() {
        let error = Config::parse(r#"disabled = "terminal""#).unwrap_err();
        assert!(!error.is_empty() && !error.contains('\n'), "{error}");
    }
}
