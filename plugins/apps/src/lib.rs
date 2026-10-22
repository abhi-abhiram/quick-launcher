//! The bundled `apps` plugin (*Apps are a plugin* in `docs/protocol.md`):
//! installed apps, read from desktop entries in the XDG data dirs, each
//! launched by its desktop entry id through the core's platform.
//!
//! The binary reads its settings from the environment ([`Env::from_process`]).
//! Tests build an [`Env`] by hand and call [`find_apps`].

mod desktop;

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use quick_launcher_plugin::{Action, Icon, Item, Plugin, Run};

pub use desktop::Locale;

/// Where and how to look for apps.
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// Data dirs, highest precedence first. Desktop entries are under
    /// `applications/` in each.
    pub data_dirs: Vec<PathBuf>,
    /// The messages locale, for `Name[de]` and the like.
    pub locale: Locale,
    /// `XDG_CURRENT_DESKTOP`, for `OnlyShowIn` and `NotShowIn`.
    pub desktops: Vec<String>,
    /// `PATH`, for `TryExec`.
    pub path: Vec<PathBuf>,
}

impl Env {
    /// The process environment, with the XDG defaults where a variable is
    /// unset or empty.
    pub fn from_process() -> Env {
        let var = |name: &str| env::var(name).ok().filter(|value| !value.is_empty());
        let home = var("HOME").map(PathBuf::from).unwrap_or_default();
        let mut data_dirs = vec![
            var("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/share")),
        ];
        let system = var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        data_dirs.extend(
            system
                .split(':')
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
        );
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .find_map(var)
            .map(|name| Locale::parse(&name))
            .unwrap_or_default();
        Env {
            data_dirs,
            locale,
            desktops: var("XDG_CURRENT_DESKTOP")
                .map(|value| value.split(':').map(String::from).collect())
                .unwrap_or_default(),
            path: env::var_os("PATH")
                .map(|path| env::split_paths(&path).collect())
                .unwrap_or_default(),
        }
    }
}

/// One installed app, as the plugin lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    /// The desktop entry id, such as `firefox.desktop` or
    /// `org.gnome.Nautilus.desktop`. Also the item id.
    pub id: String,
    /// `Name`, localized.
    pub name: String,
    /// `Comment`, else `GenericName`, localized.
    pub detail: Option<String>,
    /// `Keywords`, localized.
    pub keywords: Vec<String>,
    pub icon: Option<AppIcon>,
}

/// An app's `Icon`: a themed icon name, or an absolute path to an image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppIcon {
    Name(String),
    Path(PathBuf),
}

impl App {
    /// The item the plugin sends: one action, launching the app by its id.
    pub fn into_item(self) -> Item {
        let mut item = Item::new(&self.id, self.name).keywords(self.keywords);
        if let Some(detail) = self.detail {
            item = item.detail(detail);
        }
        if let Some(icon) = self.icon {
            item = item.icon(match icon {
                AppIcon::Name(name) => Icon::name(name),
                AppIcon::Path(path) => Icon::path(path),
            });
        }
        item.action(Action::new("launch", "Launch").run(Run::app(self.id)))
    }
}

/// Every app to show, sorted by name. An entry in an earlier data dir hides
/// one with the same id in a later one, even when the earlier one is hidden
/// itself: that's how a user removes a system app from menus.
///
/// Unreadable files and dirs are reported on stderr and skipped, so one bad
/// entry doesn't empty the list.
pub fn find_apps(env: &Env) -> Vec<App> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for dir in &env.data_dirs {
        for (id, path) in desktop_files(&dir.join("applications")) {
            if !seen.insert(id.clone()) {
                continue;
            }
            match fs::read_to_string(&path) {
                Ok(text) => apps.extend(app(id, &text, env)),
                Err(e) => eprintln!("apps: reading {}: {e}", path.display()),
            }
        }
    }
    apps.sort_by(|a, b| (a.name.to_lowercase(), &a.id).cmp(&(b.name.to_lowercase(), &b.id)));
    apps
}

/// The app a desktop entry describes, or `None` when it isn't one to show.
fn app(id: String, text: &str, env: &Env) -> Option<App> {
    let entry = desktop::Entry::parse(text)?;
    if entry.string("Type").as_deref() != Some("Application")
        || entry.flag("NoDisplay")
        || entry.flag("Hidden")
    {
        return None;
    }
    let only = entry.list("OnlyShowIn");
    if !only.is_empty() && !env.desktops.iter().any(|d| only.contains(d)) {
        return None;
    }
    let not = entry.list("NotShowIn");
    if env.desktops.iter().any(|d| not.contains(d)) {
        return None;
    }
    if let Some(program) = entry.string("TryExec")
        && !installed(&program, &env.path)
    {
        return None;
    }
    let locale = &env.locale;
    let name = entry
        .localized("Name", locale)
        .filter(|name| !name.is_empty())?;
    let detail = entry
        .localized("Comment", locale)
        .or_else(|| entry.localized("GenericName", locale))
        .filter(|detail| !detail.is_empty());
    let icon = entry
        .localized("Icon", locale)
        .filter(|icon| !icon.is_empty())
        .map(|icon| {
            if Path::new(&icon).is_absolute() {
                AppIcon::Path(icon.into())
            } else {
                AppIcon::Name(icon)
            }
        });
    Some(App {
        id,
        name,
        detail,
        keywords: entry.localized_list("Keywords", locale),
        icon,
    })
}

/// Desktop files under `root`, recursively, with their ids: the path below
/// `root` with `/` turned into `-`. Sorted, so the result doesn't depend on
/// the order the filesystem lists them in.
fn desktop_files(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    walk(root, "", &mut out);
    out
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            eprintln!("apps: reading {}: {e}", dir.display());
            return;
        }
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if path.is_dir() {
            walk(&path, &format!("{prefix}{name}-"), out);
        } else if name.ends_with(".desktop") {
            out.push((format!("{prefix}{name}"), path));
        }
    }
}

/// Whether `TryExec`'s program exists: an absolute path as given, else a
/// file of that name somewhere on `PATH`.
fn installed(program: &str, path: &[PathBuf]) -> bool {
    let program = Path::new(program);
    if program.is_absolute() {
        return program.is_file();
    }
    path.iter().any(|dir| dir.join(program).is_file())
}

/// The plugin: lists [`find_apps`] for its [`Env`].
pub struct Apps(pub Env);

impl Plugin for Apps {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        Ok(find_apps(&self.0).into_iter().map(App::into_item).collect())
    }
}
