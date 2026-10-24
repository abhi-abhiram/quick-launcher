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
    /// unset or empty. Relative data dirs are ignored, as XDG says.
    pub fn from_process() -> Env {
        let var = |name: &str| env::var(name).ok().filter(|value| !value.is_empty());
        let paths = |name: &str| {
            env::var_os(name)
                .map(|value| env::split_paths(&value).collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .filter(|path| path.is_absolute())
                .collect::<Vec<_>>()
        };
        let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let mut data_dirs = paths("XDG_DATA_HOME");
        data_dirs.truncate(1);
        if data_dirs.is_empty() {
            data_dirs.push(home.join(".local/share"));
        }
        let system = paths("XDG_DATA_DIRS");
        if system.is_empty() {
            data_dirs.extend(["/usr/local/share", "/usr/share"].map(PathBuf::from));
        } else {
            data_dirs.extend(system);
        }
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
/// entry doesn't empty the list. An unreadable entry hides nothing: the next
/// one with its id is used instead.
pub fn find_apps(env: &Env) -> Vec<App> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for dir in &env.data_dirs {
        for (id, path) in desktop_files(&dir.join("applications")) {
            if seen.contains(&id) {
                continue;
            }
            match fs::read_to_string(&path) {
                Ok(text) => {
                    seen.insert(id.clone());
                    apps.extend(app(id, &text, env));
                }
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
    if !shown_in(&entry, &env.desktops) {
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
        .filter(|detail| !detail.is_empty())
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

/// `OnlyShowIn` and `NotShowIn` for the current desktops: the first desktop
/// named in either list decides. When none is, an entry with `OnlyShowIn`
/// is hidden and any other is shown.
fn shown_in(entry: &desktop::Entry, desktops: &[String]) -> bool {
    let (only, not) = (entry.list("OnlyShowIn"), entry.list("NotShowIn"));
    for desktop in desktops {
        if only.contains(desktop) {
            return true;
        }
        if not.contains(desktop) {
            return false;
        }
    }
    only.is_empty()
}

/// Desktop files under `root`, recursively, with their ids: the path below
/// `root` with `/` turned into `-`. Sorted, so the result doesn't depend on
/// the order the filesystem lists them in. Symlinked files are read, but
/// symlinked folders aren't followed, so a link cycle can't hang the walk.
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
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if meta.is_dir() {
            walk(&path, &format!("{prefix}{name}-"), out);
        } else if name.ends_with(".desktop") {
            out.push((format!("{prefix}{name}"), path));
        }
    }
}

/// Whether `TryExec`'s program is installed: an absolute path as given, else
/// a file of that name somewhere on `PATH`, executable either way.
fn installed(program: &str, path: &[PathBuf]) -> bool {
    let program = Path::new(program);
    if program.is_absolute() {
        return executable(program);
    }
    path.iter().any(|dir| executable(&dir.join(program)))
}

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// The plugin: lists [`find_apps`] for its [`Env`].
pub struct Apps(pub Env);

impl Plugin for Apps {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        Ok(find_apps(&self.0).into_iter().map(App::into_item).collect())
    }
}
