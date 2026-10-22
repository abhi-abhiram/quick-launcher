//! Builders for what a plugin returns: items, their actions and icons
//! (*Items*, *Actions* and *Icons* in `docs/protocol.md`), and the outcome of
//! a `plugin` action.

use std::path::Path;

use quick_launcher_wire as wire;
use quick_launcher_wire::matcher::Searchable;

pub use quick_launcher_wire::After;

/// One result a plugin offers. Needs at least one [`Action`]; the first is the
/// default and the second the alternate.
///
/// ```
/// use quick_launcher_plugin::{Action, Item, Run};
///
/// let item = Item::new("/home/me/notes", "notes")
///     .detail("~/notes")
///     .keywords(["journal"])
///     .action(Action::new("open", "Open").run(Run::open("/home/me/notes")));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Item(wire::Item);

impl Item {
    /// An item with no actions yet. `id` must be stable within the plugin: it
    /// comes back in [`Activate::item`](crate::Activate::item).
    ///
    /// Add at least one [`action`](Item::action): the core drops an item
    /// without one. Debug builds of a plugin fail the request instead, so
    /// the mistake shows.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Item(wire::Item {
            id: id.into(),
            label: label.into(),
            detail: None,
            icon: None,
            badge: None,
            keywords: Vec::new(),
            disabled: false,
            score: None,
            actions: Vec::new(),
        })
    }

    /// The second line. Filtered on, ranked below label matches.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.0.detail = Some(detail.into());
        self
    }

    /// Defaults to the plugin's icon.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.0.icon = Some(icon.into().0);
        self
    }

    /// A short tag after the label, such as `pinned` or `missing`.
    pub fn badge(mut self, badge: impl Into<String>) -> Self {
        self.0.badge = Some(badge.into());
        self
    }

    /// Extra filter terms, not shown. Adds to any given before.
    pub fn keywords<I, S>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.0.keywords.extend(keywords.into_iter().map(Into::into));
        self
    }

    /// Shown dimmed, and the first action can't run. The others still can, so
    /// a missing pinned folder can still be unpinned.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.0.disabled = disabled;
        self
    }

    /// `query` plugins only: higher ranks first.
    pub fn score(mut self, score: f64) -> Self {
        self.0.score = Some(score);
        self
    }

    /// Adds an action after the ones given before.
    pub fn action(mut self, action: Action) -> Self {
        self.0.actions.push(action.0);
        self
    }

    pub(crate) fn into_wire(self) -> wire::Item {
        debug_assert!(
            !self.0.actions.is_empty(),
            "item {:?} has no actions, so the core would drop it",
            self.0.id
        );
        self.0
    }
}

impl Searchable for Item {
    fn label(&self) -> &str {
        &self.0.label
    }

    fn detail(&self) -> Option<&str> {
        self.0.detail.as_deref()
    }

    fn keywords(&self) -> &[String] {
        &self.0.keywords
    }
}

/// Something the user can do with an item.
///
/// Until [`run`](Action::run) gives it something else to do, an action is a
/// `plugin` action: choosing it sends [`Plugin::activate`](crate::Plugin::activate)
/// with this action's id.
#[derive(Debug, Clone, PartialEq)]
pub struct Action(wire::Action);

impl Action {
    /// `id` comes back in [`Activate::action`](crate::Activate::action);
    /// `title` names the action wherever the frontend lists actions.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        let id = id.into();
        Action(wire::Action {
            run: wire::Run::Plugin(id.clone()),
            id: Some(id),
            title: title.into(),
            key: None,
            after: After::Close,
        })
    }

    /// What the core runs, through the platform, when the user chooses this.
    pub fn run(mut self, run: Run) -> Self {
        self.0.run = run.0;
        self
    }

    /// Makes this a `plugin` action, sent back to the plugin's
    /// [`activate`](crate::Plugin::activate). That's what an action does
    /// unless [`run`](Action::run) was given, so this only says so.
    pub fn plugin(mut self) -> Self {
        let id = self.0.id.clone().unwrap_or_default();
        self.0.run = wire::Run::Plugin(id);
        self
    }

    /// A suggested shortcut such as `"ctrl+p"`, which frontends may bind.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.0.key = Some(key.into());
        self
    }

    /// Whether the launcher closes (the default) or refreshes after the
    /// action ran. Ignored for `plugin` actions, whose [`Outcome`] decides.
    pub fn after(mut self, after: After) -> Self {
        self.0.after = after;
        self
    }
}

/// What an action runs. The core runs it through its platform, so a plugin
/// says *what* to run and never *how* on a given OS.
#[derive(Debug, Clone, PartialEq)]
pub struct Run(wire::Run);

impl Run {
    /// Starts `argv` detached, never through a shell. A relative program path
    /// resolves against the plugin folder.
    pub fn exec<I, S>(argv: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Run(wire::Run::Exec {
            argv: argv.into_iter().map(Into::into).collect(),
            cwd: None,
        })
    }

    /// Starts `argv` in the user's terminal, never through a shell.
    pub fn terminal<I, S>(argv: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Run(wire::Run::Terminal {
            argv: argv.into_iter().map(Into::into).collect(),
            cwd: None,
        })
    }

    /// Launches an installed app by its platform id (`firefox.desktop`).
    pub fn app(id: impl Into<String>) -> Self {
        Run(wire::Run::App(id.into()))
    }

    /// Opens a URL or path with its default handler.
    pub fn open(target: impl Into<String>) -> Self {
        Run(wire::Run::Open(target.into()))
    }

    /// Puts text on the clipboard.
    pub fn copy(text: impl Into<String>) -> Self {
        Run(wire::Run::Copy(text.into()))
    }

    /// The working directory for [`exec`](Run::exec) and
    /// [`terminal`](Run::terminal); the home folder when not given. Other
    /// forms run no command, so they ignore it.
    pub fn cwd(mut self, dir: impl AsRef<Path>) -> Self {
        if let wire::Run::Exec { cwd, .. } | wire::Run::Terminal { cwd, .. } = &mut self.0 {
            *cwd = Some(dir.as_ref().to_string_lossy().into_owned());
        }
        self
    }
}

/// An item's icon. A string is always a glyph, however short or long.
///
/// ```
/// use quick_launcher_plugin::{Icon, Item};
///
/// let glyph = Item::new("a", "A").icon("󰐃");
/// let named = Item::new("b", "B").icon(Icon::name("folder"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icon(wire::Icon);

impl Icon {
    /// A Nerd Font glyph or emoji.
    pub fn glyph(glyph: impl Into<String>) -> Self {
        Icon(wire::Icon::Glyph(glyph.into()))
    }

    /// A platform icon name (a freedesktop icon name on Omarchy).
    pub fn name(name: impl Into<String>) -> Self {
        Icon(wire::Icon::Name(name.into()))
    }

    /// An image file.
    pub fn path(path: impl AsRef<Path>) -> Self {
        Icon(wire::Icon::Path(
            path.as_ref().to_string_lossy().into_owned(),
        ))
    }
}

impl From<&str> for Icon {
    fn from(glyph: &str) -> Self {
        Icon::glyph(glyph)
    }
}

impl From<String> for Icon {
    fn from(glyph: String) -> Self {
        Icon::glyph(glyph)
    }
}

/// What the launcher does after a `plugin` action ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome(wire::ActivateResult);

impl Outcome {
    /// Close the launcher.
    pub fn close() -> Self {
        Outcome(wire::ActivateResult::default())
    }

    /// Keep the launcher open and ask for the mode's items again, so a change
    /// such as a pin shows at once.
    pub fn refresh() -> Self {
        Outcome(wire::ActivateResult {
            close: false,
            refresh: true,
            query: None,
        })
    }

    /// Keep the launcher open and replace the typed text with `text`.
    pub fn query(text: impl Into<String>) -> Self {
        Outcome(wire::ActivateResult {
            close: false,
            refresh: false,
            query: Some(text.into()),
        })
    }

    pub(crate) fn into_wire(self) -> wire::ActivateResult {
        self.0
    }
}
