//! Items, actions and icons, as plugins send them (*Items*, *Actions* and
//! *Icons* in `docs/protocol.md`).

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::one_of::exactly_one;

/// One result a plugin offers.
///
/// `id`, `label` and `actions` are required. Dropping an item that lacks them
/// while keeping the rest of a response is the core's job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// Stable within the plugin; sent back in `activate`.
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Defaults to the plugin's icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<Icon>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    /// Extra filter terms, not shown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// Shown dimmed; the first action can't run, the others can.
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
    /// `query` plugins only: higher ranks first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// The first is the default action, the second the alternate.
    pub actions: Vec<Action>,
}

/// Something the user can do with an item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    /// Sent back in `activate`. When absent, the action's index is its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub title: String,
    /// A suggested shortcut such as `"ctrl+d"`, which frontends may bind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub run: Run,
    /// Ignored for `plugin` runs, which decide in their result.
    #[serde(default, skip_serializing_if = "After::is_close")]
    pub after: After,
}

/// What an action does: exactly one of the six forms.
#[derive(Debug, Clone, PartialEq)]
pub enum Run {
    /// Start argv detached. A relative program path (one with a `/`)
    /// resolves against the plugin folder; `cwd` defaults to the home folder.
    Exec {
        argv: Vec<String>,
        cwd: Option<String>,
    },
    /// Start argv in the user's terminal; `cwd` defaults to the home folder.
    Terminal {
        argv: Vec<String>,
        cwd: Option<String>,
    },
    /// Launch an installed app by its platform id (`firefox.desktop`).
    App(String),
    /// Open a URL or path with its default handler.
    Open(String),
    /// Put text on the clipboard.
    Copy(String),
    /// Send `activate` with this action id to the plugin.
    Plugin(String),
}

/// What happens after a non-`plugin` action ran.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum After {
    #[default]
    Close,
    Refresh,
}

impl After {
    fn is_close(&self) -> bool {
        *self == After::Close
    }
}

/// An item's or a plugin's icon.
///
/// On the wire a plain string is always a glyph, however long; names and
/// files use the object forms `{"name": …}` and `{"path": …}`, and
/// `{"glyph": …}` is the long form of a string. A glyph is written back as
/// the short form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Icon {
    /// A Nerd Font glyph or emoji.
    Glyph(String),
    /// A platform icon name (a freedesktop icon name on Omarchy).
    Name(String),
    /// An image file.
    Path(String),
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Every key any `run` form uses. Deserializing through it ignores unknown
/// keys and lets us reject zero or several forms with a clear message.
#[derive(Default, Serialize, Deserialize)]
struct RunFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exec: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    open: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    copy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
}

impl Serialize for Run {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = RunFields::default();
        match self {
            Run::Exec { argv, cwd } => {
                fields.exec = Some(argv.clone());
                fields.cwd = cwd.clone();
            }
            Run::Terminal { argv, cwd } => {
                fields.terminal = Some(argv.clone());
                fields.cwd = cwd.clone();
            }
            Run::App(id) => fields.app = Some(id.clone()),
            Run::Open(target) => fields.open = Some(target.clone()),
            Run::Copy(text) => fields.copy = Some(text.clone()),
            Run::Plugin(action) => fields.plugin = Some(action.clone()),
        }
        fields.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Run {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let f = RunFields::deserialize(deserializer)?;
        let mut forms = Vec::new();
        if let Some(argv) = f.exec {
            forms.push(Run::Exec {
                argv,
                cwd: f.cwd.clone(),
            });
        }
        if let Some(argv) = f.terminal {
            forms.push(Run::Terminal {
                argv,
                cwd: f.cwd.clone(),
            });
        }
        forms.extend(f.app.map(Run::App));
        forms.extend(f.open.map(Run::Open));
        forms.extend(f.copy.map(Run::Copy));
        forms.extend(f.plugin.map(Run::Plugin));
        exactly_one(forms, "run", "exec, terminal, app, open, copy or plugin")
            .map_err(D::Error::custom)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct IconFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    glyph: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum IconRepr {
    Glyph(String),
    Object(IconFields),
}

impl Serialize for Icon {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = IconFields::default();
        match self {
            Icon::Glyph(glyph) => return serializer.serialize_str(glyph),
            Icon::Name(name) => fields.name = Some(name.clone()),
            Icon::Path(path) => fields.path = Some(path.clone()),
        }
        fields.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Icon {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let f = match IconRepr::deserialize(deserializer).map_err(|_| {
            D::Error::custom("icon must be a string or a glyph, name or path object")
        })? {
            IconRepr::Glyph(glyph) => return Ok(Icon::Glyph(glyph)),
            IconRepr::Object(f) => f,
        };
        let mut forms: Vec<Icon> = Vec::new();
        forms.extend(f.glyph.map(Icon::Glyph));
        forms.extend(f.name.map(Icon::Name));
        forms.extend(f.path.map(Icon::Path));
        exactly_one(forms, "icon object", "glyph, name or path").map_err(D::Error::custom)
    }
}
