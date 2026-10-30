//! A plugin's `plugin.toml` (*Manifest*, *Kinds* and *Lifetimes* in
//! `docs/protocol.md`).

use std::fmt;

use quick_launcher_wire::{Action, Icon, PROTOCOL};
use serde::Deserialize;

use crate::action::PluginEnv;

/// A manifest that parsed and passed every rule. Keys the core doesn't know
/// are ignored, so newer manifests load in older cores.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub id: String,
    /// Shown in hints and the mode chip.
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<Icon>,
    /// At least one. Matched case-insensitively.
    pub prefixes: Vec<String>,
    /// Shown in the empty search field.
    pub placeholder: Option<String>,
    /// The binary's file name in the plugin folder.
    pub bin: String,
    pub kind: Kind,
    /// `list` plugins only: also searched in the plain search.
    pub global: bool,
    pub lifetime: Lifetime,
    pub env: PluginEnv,
    /// Per request.
    pub timeout_ms: u64,
    /// `list` plugins only: an item built from the typed text, shown first.
    pub query_item: Option<QueryItem>,
}

/// How the core asks a plugin for items.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Every item once, filtered by the core.
    #[default]
    List,
    /// Items for the text, on every keystroke.
    Query,
}

/// How long a plugin process lives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lifetime {
    /// One process per request.
    #[default]
    Oneshot,
    /// Started on first use and kept running.
    Persistent,
}

/// A manifest's `[query_item]`. `{query}` stays in its strings as written;
/// the session fills it in.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct QueryItem {
    pub label: String,
    pub detail: Option<String>,
    pub icon: Option<Icon>,
    pub badge: Option<String>,
    /// At least one; the first is the default action.
    pub actions: Vec<Action>,
}

/// Why a manifest was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// Built for another wire format major, so the plugin needs a rebuild.
    Protocol(i64),
    /// Not TOML, or a key is missing, has the wrong type or breaks a rule.
    Invalid(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Protocol(found) => write!(
                f,
                "built for protocol {found}, but this core speaks protocol {PROTOCOL}; \
                 rebuild the plugin against the current SDK"
            ),
            ManifestError::Invalid(reason) => f.write_str(reason),
        }
    }
}

/// The keys as written, before the rules that serde can't express.
#[derive(Deserialize)]
struct Raw {
    id: String,
    name: String,
    description: Option<String>,
    icon: Option<Icon>,
    prefixes: Vec<String>,
    placeholder: Option<String>,
    bin: String,
    #[serde(default)]
    kind: Kind,
    #[serde(default)]
    global: bool,
    #[serde(default)]
    lifetime: Lifetime,
    #[serde(default)]
    env: Env,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    query_item: Option<QueryItem>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Env {
    #[default]
    Session,
    Login,
}

fn default_timeout_ms() -> u64 {
    2000
}

impl Manifest {
    /// Parses and checks `plugin.toml`'s text.
    ///
    /// `protocol` is checked first, on its own, so a manifest written for
    /// another protocol gets the rebuild message even if its other keys
    /// changed too.
    pub fn parse(text: &str) -> Result<Manifest, ManifestError> {
        let table: toml::Table = toml::from_str(text).map_err(invalid)?;
        match table.get("protocol") {
            None => return Err(ManifestError::Invalid("missing key `protocol`".into())),
            Some(toml::Value::Integer(n)) if *n == i64::from(PROTOCOL) => {}
            Some(toml::Value::Integer(n)) => return Err(ManifestError::Protocol(*n)),
            Some(_) => {
                return Err(ManifestError::Invalid(
                    "`protocol` must be an integer".into(),
                ));
            }
        }
        let raw: Raw = table.try_into().map_err(invalid)?;
        raw.check()
    }
}

impl Raw {
    fn check(self) -> Result<Manifest, ManifestError> {
        if !is_valid_id(&self.id) {
            return Err(rule(format!("id {:?} must match [a-z0-9-]+", self.id)));
        }
        if self.prefixes.is_empty() {
            return Err(rule("`prefixes` must list at least one prefix"));
        }
        if self.prefixes.iter().any(|prefix| prefix.trim().is_empty()) {
            return Err(rule("a prefix can't be empty or only whitespace"));
        }
        if !is_file_name(&self.bin) {
            return Err(rule(format!(
                "bin {:?} must be a file name in the plugin folder",
                self.bin
            )));
        }
        if self.timeout_ms == 0 {
            return Err(rule("`timeout_ms` must be more than 0"));
        }
        if self.kind == Kind::Query {
            if self.global {
                return Err(rule("`global = true` is only for kind = \"list\""));
            }
            if self.query_item.is_some() {
                return Err(rule("`[query_item]` is only for kind = \"list\""));
            }
        }
        if let Some(item) = &self.query_item
            && item.actions.is_empty()
        {
            return Err(rule("`[query_item]` needs at least one action"));
        }
        Ok(Manifest {
            id: self.id,
            name: self.name,
            description: self.description,
            icon: self.icon,
            prefixes: self.prefixes,
            placeholder: self.placeholder,
            bin: self.bin,
            kind: self.kind,
            global: self.global,
            lifetime: self.lifetime,
            env: match self.env {
                Env::Session => PluginEnv::Session,
                Env::Login => PluginEnv::Login,
            },
            timeout_ms: self.timeout_ms,
            query_item: self.query_item,
        })
    }
}

/// `[a-z0-9-]+`.
pub(crate) fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A plain file name: no separators, not `.` or `..`.
fn is_file_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/')
}

fn invalid(error: toml::de::Error) -> ManifestError {
    ManifestError::Invalid(one_line(&error))
}

/// toml's message without the source excerpt its `Display` adds, so the log
/// gets one line.
pub(crate) fn one_line(error: &toml::de::Error) -> String {
    error
        .message()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn rule(reason: impl Into<String>) -> ManifestError {
    ManifestError::Invalid(reason.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quick_launcher_wire::Run;
    use std::path::Path;

    const MINIMAL: &str = r#"
        protocol = 1
        id = "repos"
        name = "Repos"
        prefixes = ["repo:"]
        bin = "repos"
    "#;

    fn parse(text: &str) -> Manifest {
        Manifest::parse(text).unwrap()
    }

    fn reason(text: &str) -> String {
        match Manifest::parse(text) {
            Err(ManifestError::Invalid(reason)) => reason,
            other => panic!("expected an invalid manifest, got {other:?}"),
        }
    }

    /// MINIMAL with `extra` appended.
    fn with(extra: &str) -> String {
        format!("{MINIMAL}\n{extra}")
    }

    #[test]
    fn a_minimal_manifest_takes_the_defaults() {
        assert_eq!(
            parse(MINIMAL),
            Manifest {
                id: "repos".into(),
                name: "Repos".into(),
                description: None,
                icon: None,
                prefixes: vec!["repo:".into()],
                placeholder: None,
                bin: "repos".into(),
                kind: Kind::List,
                global: false,
                lifetime: Lifetime::Oneshot,
                env: PluginEnv::Session,
                timeout_ms: 2000,
                query_item: None,
            }
        );
    }

    #[test]
    fn the_spec_manifest_parses_in_full() {
        let manifest = parse(
            r#"
            protocol = 1
            id = "pi"
            name = "pi"
            description = "Open pi in a project"
            icon = "π"
            prefixes = ["pi:"]
            placeholder = "Open pi in…"
            bin = "pi-plugin"
            kind = "list"
            global = false
            lifetime = "persistent"
            env = "login"
            timeout_ms = 500

            [query_item]
            label = "{query}"
            detail = "Run in a new terminal"
            actions = [{ title = "Run", run = { plugin = "run" } }]
            "#,
        );
        assert_eq!(
            manifest.description.as_deref(),
            Some("Open pi in a project")
        );
        assert_eq!(manifest.icon, Some(Icon::Glyph("π".into())));
        assert_eq!(manifest.placeholder.as_deref(), Some("Open pi in…"));
        assert_eq!(manifest.lifetime, Lifetime::Persistent);
        assert_eq!(manifest.env, PluginEnv::Login);
        assert_eq!(manifest.timeout_ms, 500);
        let item = manifest.query_item.unwrap();
        assert_eq!(item.label, "{query}");
        assert_eq!(item.detail.as_deref(), Some("Run in a new terminal"));
        assert_eq!(item.actions[0].title, "Run");
        assert_eq!(item.actions[0].run, Run::Plugin("run".into()));
    }

    #[test]
    fn icons_take_every_form() {
        for (toml, icon) in [
            (r#"icon = """#, Icon::Glyph("".into())),
            (r#"icon = { name = "folder" }"#, Icon::Name("folder".into())),
            (r#"icon = { path = "/i.svg" }"#, Icon::Path("/i.svg".into())),
        ] {
            assert_eq!(parse(&with(toml)).icon, Some(icon));
        }
    }

    #[test]
    fn every_bundled_manifest_parses() {
        let plugins = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(&plugins).unwrap() {
            let folder = entry.unwrap().path();
            let file = folder.join("plugin.toml");
            if let Ok(text) = std::fs::read_to_string(&file) {
                let manifest =
                    Manifest::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
                assert_eq!(folder.file_name().unwrap(), manifest.id.as_str());
                ids.push(manifest.id);
            }
        }
        ids.sort();
        assert_eq!(ids, ["apps", "pi", "projects", "terminal"]);
    }

    #[test]
    fn the_bundled_apps_manifest_parses() {
        let text = include_str!("../../../../plugins/apps/plugin.toml");
        let manifest = parse(text);
        assert_eq!(manifest.id, "apps");
        assert!(manifest.global);
    }

    #[test]
    fn the_bundled_pi_manifest_parses() {
        let text = include_str!("../../../../plugins/pi/plugin.toml");
        let manifest = parse(text);
        assert_eq!(manifest.id, "pi");
        assert_eq!(manifest.prefixes, ["pi:"]);
        assert_eq!(manifest.bin, "pi-plugin");
        assert_eq!(manifest.kind, Kind::List);
        assert_eq!(manifest.lifetime, Lifetime::Oneshot);
        assert_eq!(manifest.env, PluginEnv::Login);
        assert!(!manifest.global);
        assert_eq!(manifest.query_item, None);
    }

    #[test]
    fn the_bundled_terminal_manifest_parses() {
        let text = include_str!("../../../../plugins/terminal/plugin.toml");
        let manifest = parse(text);
        assert_eq!(manifest.id, "terminal");
        assert_eq!(manifest.prefixes, [">", "term:"]);
        assert_eq!(manifest.bin, "terminal-plugin");
        assert_eq!(manifest.kind, Kind::List);
        assert_eq!(manifest.lifetime, Lifetime::Oneshot);
        assert_eq!(manifest.env, PluginEnv::Session);
        assert!(!manifest.global);
        let item = manifest.query_item.unwrap();
        assert_eq!(item.label, "{query}");
        assert_eq!(item.detail.as_deref(), Some("Run in a new terminal"));
        assert_eq!(item.actions[0].id.as_deref(), Some("run"));
        assert_eq!(item.actions[0].run, Run::Plugin("run".into()));
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let manifest = parse(&with(
            r#"
            future = "yes"
            [future_table]
            x = 1
            "#,
        ));
        assert_eq!(manifest.id, "repos");
    }

    #[test]
    fn required_keys_are_required() {
        for key in ["protocol", "id", "name", "prefixes", "bin"] {
            let text: String = MINIMAL
                .lines()
                .filter(|line| !line.trim_start().starts_with(key))
                .collect::<Vec<_>>()
                .join("\n");
            let reason = reason(&text);
            assert!(reason.contains(key), "{key}: {reason}");
        }
    }

    #[test]
    fn another_protocol_asks_for_a_rebuild() {
        let text = MINIMAL.replace("protocol = 1", "protocol = 2");
        let err = Manifest::parse(&text).unwrap_err();
        assert_eq!(err, ManifestError::Protocol(2));
        assert!(err.to_string().contains("rebuild"));
    }

    #[test]
    fn protocol_is_checked_before_the_other_keys() {
        // A future manifest that dropped `name` still gets the rebuild message.
        let text = r#"
            protocol = 2
            id = "repos"
        "#;
        assert_eq!(Manifest::parse(text), Err(ManifestError::Protocol(2)));
    }

    #[test]
    fn the_id_must_match_the_pattern() {
        for id in ["", "Repos", "my_repos", "repos!", "ré"] {
            let text = MINIMAL.replace(r#"id = "repos""#, &format!("id = {id:?}"));
            assert!(reason(&text).contains("[a-z0-9-]+"), "{id:?}");
        }
        let text = MINIMAL.replace(r#"id = "repos""#, r#"id = "my-repos-2""#);
        assert_eq!(parse(&text).id, "my-repos-2");
    }

    #[test]
    fn prefixes_need_at_least_one_that_is_not_blank() {
        let none = MINIMAL.replace(r#"["repo:"]"#, "[]");
        assert!(reason(&none).contains("at least one"));
        let blank = MINIMAL.replace(r#"["repo:"]"#, r#"["repo:", " "]"#);
        assert!(reason(&blank).contains("empty"));
    }

    #[test]
    fn bin_must_be_a_file_name() {
        for bin in ["", "..", "bin/repos", "/usr/bin/repos"] {
            let text = MINIMAL.replace(r#"bin = "repos""#, &format!("bin = {bin:?}"));
            assert!(reason(&text).contains("file name"), "{bin:?}");
        }
    }

    #[test]
    fn global_and_query_item_are_only_for_list_plugins() {
        let query = with(r#"kind = "query""#);
        assert_eq!(parse(&query).kind, Kind::Query);
        assert!(!parse(&with("kind = \"query\"\nglobal = false")).global);
        assert!(reason(&with("kind = \"query\"\nglobal = true")).contains("global"));
        let query_item = with(
            r#"
            kind = "query"
            [query_item]
            label = "{query}"
            actions = [{ title = "Copy", run = { copy = "{query}" } }]
            "#,
        );
        assert!(reason(&query_item).contains("query_item"));
    }

    #[test]
    fn a_query_item_needs_an_action() {
        let text = with(
            r#"
            [query_item]
            label = "{query}"
            actions = []
            "#,
        );
        assert!(reason(&text).contains("at least one action"));
    }

    #[test]
    fn bad_values_are_refused_on_one_line() {
        for extra in [
            r#"kind = "stream""#,
            r#"lifetime = "forever""#,
            r#"env = "shell""#,
            "timeout_ms = -1",
            "timeout_ms = 0",
            r#"global = "yes""#,
        ] {
            let reason = reason(&with(extra));
            assert!(
                !reason.is_empty() && !reason.contains('\n'),
                "{extra}: {reason}"
            );
        }
        assert!(!reason("not toml = = 1").contains('\n'));
    }
}
