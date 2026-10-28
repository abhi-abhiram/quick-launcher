//! The bundled `projects` plugin: open a terminal in a saved project folder,
//! and add folders to the list with `cd`-style completion.
//!
//! The mode never opens a folder the user hasn't saved. A name, or part of
//! a path that doesn't start with `~` or `/`, searches the saved projects
//! only. A path starting with `~` or `/` lists the saved projects under it,
//! then the folders it could complete to, as `cd` would: each folder's
//! `complete` is its path with a trailing `/`, so Tab moves into it, and
//! choosing one saves it.
//!
//! Saved projects are kept most recently added or opened first. The binary
//! reads its settings from the environment ([`Env::from_process`]). Tests
//! build an [`Env`] by hand.

use std::env;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use quick_launcher_plugin::{Action, Activate, Item, Outcome, Plugin, Query, Run, matcher};
use serde_json::{Value, json};

/// The project and folder icon.
pub const ICON: &str = "󰉋";

/// The icon of the rows that add a folder.
pub const ADD_ICON: &str = "󰐕";

/// What a project's terminal runs: the user's shell. The core needs a
/// command, and the terminal would start the same shell without one.
pub const SHELL: [&str; 3] = ["sh", "-c", r#"exec "${SHELL:-bash}""#];

/// How many folder completions a path lists, which is the launcher's
/// default number of rows.
pub const COMPLETION_LIMIT: usize = 50;

/// What the "Add a project folder…" row and Tab outside a path start with.
pub const START: &str = "~/";

/// Where the plugin reads and writes.
#[derive(Debug, Clone)]
pub struct Env {
    /// `QL_STATE_DIR`, where the projects are written. Created on the first
    /// write.
    pub state_dir: PathBuf,
    /// `HOME`: what `~` stands for, and shown as `~`.
    pub home: PathBuf,
}

impl Env {
    /// The process environment. Fails when `QL_STATE_DIR` isn't set, which
    /// the core always sets. Without `HOME`, `~` paths complete nothing.
    pub fn from_process() -> anyhow::Result<Env> {
        let var = |name: &str| {
            env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Ok(Env {
            state_dir: var("QL_STATE_DIR").ok_or_else(|| {
                anyhow!("QL_STATE_DIR is not set; the launcher sets it when it starts a plugin")
            })?,
            home: var("HOME").unwrap_or_default(),
        })
    }

    /// The file holding the saved projects.
    pub fn projects_file(&self) -> PathBuf {
        self.state_dir.join("projects.json")
    }
}

/// Saved project folders, most recent first. No file means none saved. A
/// file that exists but can't be read, isn't JSON or isn't a JSON object
/// is an error; an object without a `projects` list is no projects.
pub fn read_projects(env: &Env) -> anyhow::Result<Vec<String>> {
    let file = env.projects_file();
    match fs::read_to_string(&file) {
        Ok(text) => parse_projects(&text)
            .with_context(|| format!("{} is not a valid projects file", file.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(anyhow!(e).context(format!("reading {}", file.display()))),
    }
}

/// `{"projects": [...]}`. Entries that aren't absolute paths are skipped.
fn parse_projects(text: &str) -> anyhow::Result<Vec<String>> {
    let Value::Object(file) = serde_json::from_str(text)? else {
        bail!("expected a JSON object");
    };
    let Some(Value::Array(projects)) = file.get("projects") else {
        return Ok(Vec::new());
    };
    Ok(projects
        .iter()
        .filter_map(|project| project.as_str())
        .filter(|project| project.starts_with('/'))
        .map(String::from)
        .collect())
}

/// Replaces the projects file, atomically.
pub fn write_projects(env: &Env, projects: &[String]) -> anyhow::Result<()> {
    let file = env.projects_file();
    fs::create_dir_all(&env.state_dir)
        .with_context(|| format!("creating {}", env.state_dir.display()))?;
    let temporary = env
        .state_dir
        .join(format!(".projects-{}.json", std::process::id()));
    let mut text = serde_json::to_string_pretty(&json!({ "projects": projects }))?;
    text.push('\n');
    let written = fs::write(&temporary, text).and_then(|()| fs::rename(&temporary, &file));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written.with_context(|| format!("writing {}", file.display()))
}

/// Puts `path` first, once: adding a project and opening one both make it
/// the most recent.
pub fn touch(env: &Env, path: &str) -> anyhow::Result<()> {
    let mut projects = read_projects(env)?;
    projects.retain(|project| project != path);
    projects.insert(0, path.to_owned());
    write_projects(env, &projects)
}

/// Removes `path` from the saved projects. The folder itself is untouched.
pub fn remove(env: &Env, path: &str) -> anyhow::Result<()> {
    let mut projects = read_projects(env)?;
    projects.retain(|project| project != path);
    write_projects(env, &projects)
}

/// `path` with the home folder written as `~`.
pub fn display_path(path: &str, home: &Path) -> String {
    let home = home.to_string_lossy();
    match path.strip_prefix(home.as_ref()) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
        _ => path.to_owned(),
    }
}

/// The folder's last name, or the whole path when it has none (`/`).
pub fn project_name(path: &str) -> &str {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(path)
}

/// Whether `text` is a path to complete rather than a search: it starts
/// with `~` or `/`, as a path given to `cd` would.
pub fn is_path(text: &str) -> bool {
    text.starts_with('~') || text.starts_with('/')
}

/// `~`, `~/…` and `/…` as an absolute path. `~user` and relative paths
/// aren't supported, and `~` needs a home folder.
pub fn expand(text: &str, home: &Path) -> Option<PathBuf> {
    if text.starts_with('/') {
        return Some(PathBuf::from(text));
    }
    let rest = text.strip_prefix('~')?;
    if !(rest.is_empty() || rest.starts_with('/')) || !home.is_absolute() {
        return None;
    }
    Some(home.join(rest.trim_start_matches('/')))
}

/// `path` without `.`, `..` or a trailing `/`, as `cd` reads it without
/// following links. `..` at the root stays at the root.
pub fn normalize(path: &Path) -> String {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    let text = out.to_string_lossy().into_owned();
    if text.is_empty() {
        "/".to_owned()
    } else {
        text
    }
}

/// A folder a typed path can complete to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// Its name.
    pub name: String,
    /// Its absolute path, [`normalize`]d.
    pub path: String,
    /// The typed text completed to it, ending in `/`.
    pub completion: String,
}

/// The folders `text` completes to, as `cd`'s completion lists them: the
/// folders (or links to folders) in the part of `text` up to its last `/`
/// whose names start with what follows it. Names starting with `.` only
/// when that does. A case-sensitive match wins; with none, case is ignored.
/// Sorted by name, ignoring case, and at most [`COMPLETION_LIMIT`].
pub fn completions(text: &str, home: &Path) -> Vec<Folder> {
    let text = if text == "~" { START } else { text };
    let Some(slash) = text.rfind('/') else {
        return Vec::new();
    };
    let (shown, partial) = (&text[..=slash], &text[slash + 1..]);
    let Some(dir) = expand(shown, home) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().into_string().ok()?;
            // `metadata` follows links, so a link to a folder counts.
            fs::metadata(entry.path()).ok()?.is_dir().then_some(name)
        })
        .filter(|name| partial.starts_with('.') || !name.starts_with('.'))
        .collect();
    if names.iter().any(|name| name.starts_with(partial)) {
        names.retain(|name| name.starts_with(partial));
    } else {
        let lower = partial.to_lowercase();
        names.retain(|name| name.to_lowercase().starts_with(&lower));
    }
    names.sort_by_cached_key(|name| (name.to_lowercase(), name.clone()));
    names.truncate(COMPLETION_LIMIT);
    names
        .into_iter()
        .map(|name| Folder {
            path: normalize(&dir.join(&name)),
            completion: format!("{shown}{name}/"),
            name,
        })
        .collect()
}

/// A saved project. It opens a terminal in its folder and is removed as
/// its second action. A folder that's gone is marked `missing` and can't
/// be opened, only removed. In a path, `complete` is the path to move into.
pub fn project_item(path: &str, home: &Path, complete: Option<String>) -> Item {
    let mut item = Item::new(format!("project:{path}"), project_name(path))
        .detail(display_path(path, home))
        .icon(ICON)
        .action(Action::new("open", "Open terminal"))
        .action(Action::new("remove", "Remove").key("ctrl+d"));
    if !Path::new(path).is_dir() {
        item = item.badge("missing").disabled(true);
    }
    if let Some(complete) = complete {
        item = item.complete(complete);
    }
    item
}

/// The row that starts adding a folder: it, and Tab outside a path,
/// replace the text with [`START`].
pub fn add_item() -> Item {
    Item::new("add", "Add a project folder…")
        .detail("Type a path; Tab completes folders, as cd does")
        .icon(ADD_ICON)
        .complete(START)
        .action(Action::new("start-add", "Add a project folder"))
}

/// A folder that isn't saved yet: choosing it saves it, or saves it and
/// opens a terminal there.
fn folder_item(id_path: &str, label: String, detail: String) -> Item {
    Item::new(format!("folder:{id_path}"), label)
        .detail(detail)
        .action(Action::new("add", "Add project"))
        .action(Action::new("add-open", "Add and open terminal"))
}

/// The items for `text`, the query. See the module docs.
pub fn items(projects: &[String], text: &str, home: &Path) -> Vec<Item> {
    if is_path(text) {
        return path_items(projects, text, home);
    }
    let saved: Vec<Item> = projects
        .iter()
        .map(|path| project_item(path, home, None))
        .collect();
    let mut items: Vec<Item> = matcher::filter(&saved, text)
        .into_iter()
        .map(|found| saved[found.index].clone())
        .collect();
    items.push(add_item());
    items
}

/// The items for a typed path: the saved projects under it, then the
/// folder itself when the path ends in `/` and it isn't saved, then the
/// folders it completes to that aren't saved.
fn path_items(projects: &[String], text: &str, home: &Path) -> Vec<Item> {
    // Completions keep the form the user typed: `~/…` or absolute.
    let shown = |path: &str| {
        if text.starts_with('~') {
            display_path(path, home)
        } else {
            path.to_owned()
        }
    };
    let typed = expand(text, home).map(|path| path.to_string_lossy().into_owned());
    let listed: Vec<&String> = match &typed {
        Some(typed) => {
            let folder = typed.trim_end_matches('/');
            projects
                .iter()
                .filter(|path| path.as_str() == folder || path.starts_with(typed.as_str()))
                .collect()
        }
        None => Vec::new(),
    };
    let mut items: Vec<Item> = listed
        .iter()
        .map(|path| project_item(path, home, Some(format!("{}/", shown(path)))))
        .collect();
    if text.ends_with('/') || text == "~" {
        let folder = typed
            .as_deref()
            .map(|typed| normalize(Path::new(typed)))
            .filter(|folder| Path::new(folder).is_dir() && !projects.contains(folder));
        if let Some(folder) = folder {
            items.push(
                folder_item(
                    &folder,
                    display_path(&folder, home),
                    "Add this folder as a project".to_owned(),
                )
                .icon(ADD_ICON),
            );
        }
    }
    for found in completions(text, home) {
        if projects.contains(&found.path) {
            // Listed above, unless only a case-insensitive match found it.
            if !listed.contains(&&found.path) {
                items.push(project_item(&found.path, home, Some(found.completion)));
            }
            continue;
        }
        items.push(
            folder_item(
                &found.path,
                format!("{}/", found.name),
                display_path(&found.path, home),
            )
            .icon(ICON)
            .complete(found.completion),
        );
    }
    items
}

/// Opens a terminal in `path`.
pub fn terminal_run(path: &str) -> Run {
    Run::terminal(SHELL).cwd(path)
}

/// The plugin: [`items`] for its [`Env`], and opening, adding and removing
/// projects.
pub struct Projects(pub Env);

impl Projects {
    /// The saved projects. A broken file lists none, so folders can still
    /// be completed; saving one replaces it.
    fn projects(&self) -> Vec<String> {
        read_projects(&self.0).unwrap_or_else(|e| {
            eprintln!("ignoring saved projects: {e:#}");
            Vec::new()
        })
    }
}

impl Plugin for Projects {
    fn query(&mut self, query: &Query) -> anyhow::Result<Vec<Item>> {
        Ok(items(&self.projects(), query.text(), &self.0.home))
    }

    fn activate(&mut self, activate: Activate) -> anyhow::Result<Outcome> {
        let id = activate.item.unwrap_or_default();
        let path = id
            .strip_prefix("project:")
            .or_else(|| id.strip_prefix("folder:"))
            .unwrap_or_default();
        let existing = || -> anyhow::Result<()> {
            if !Path::new(path).is_dir() {
                bail!("{} is not a folder", display_path(path, &self.0.home));
            }
            Ok(())
        };
        match activate.action.as_str() {
            "open" | "add-open" => {
                existing()?;
                // The terminal still opens when the list can't be saved.
                if let Err(e) = touch(&self.0, path) {
                    eprintln!("not saving {path:?}: {e:#}");
                }
                Ok(Outcome::close().run(terminal_run(path)))
            }
            "add" => {
                existing()?;
                touch(&self.0, path)?;
                // Back to the list, with the new project first.
                Ok(Outcome::query(""))
            }
            "remove" => {
                remove(&self.0, path)?;
                Ok(Outcome::refresh())
            }
            "start-add" => Ok(Outcome::query(START)),
            other => bail!("unknown action {other:?}"),
        }
    }
}
