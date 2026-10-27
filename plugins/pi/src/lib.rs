//! The bundled `pi` plugin (*Examples* in `docs/protocol.md`): pinned and
//! recent pi projects, opened in a terminal, with pin and unpin as `plugin`
//! actions. A port of v0's `modes/pi.py` that looks the same to users.
//!
//! The binary reads its settings from the environment ([`Env::from_process`]).
//! Tests build an [`Env`] by hand.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, anyhow, bail};
use quick_launcher_plugin::{Action, Activate, After, Item, Outcome, Plugin, Run};
use serde_json::{Value, json};

/// How many recent projects are listed, after the pinned ones.
pub const RECENT_LIMIT: usize = 20;

/// The pinned item icon, as in v0.
pub const PINNED_ICON: &str = "󰐃";

/// Where the plugin reads and writes.
#[derive(Debug, Clone)]
pub struct Env {
    /// `QL_CONFIG_DIR`, where pins are written. Created on the first write.
    pub config_dir: PathBuf,
    /// `XDG_CONFIG_HOME`, else `~/.config`: where v0 and the older
    /// pi-launcher widget kept pins.
    pub xdg_config_home: PathBuf,
    /// `HOME`: shown as `~`, and never a project.
    pub home: PathBuf,
    /// `PI_CODING_AGENT_DIR`, when set.
    pub agent_dir: Option<PathBuf>,
    /// `PI_CODING_AGENT_SESSION_DIR`, when set.
    pub session_dir: Option<PathBuf>,
}

impl Env {
    /// The process environment. Fails when `QL_CONFIG_DIR` isn't set, which
    /// the core always sets. The plugin keeps nothing in `QL_STATE_DIR`, so
    /// it isn't needed. Without `HOME`, nothing is shown as `~` and pi's
    /// default folder is relative, so no recent projects are found.
    pub fn from_process() -> anyhow::Result<Env> {
        let var = |name: &str| {
            env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let home = var("HOME").unwrap_or_default();
        Ok(Env {
            config_dir: var("QL_CONFIG_DIR").ok_or_else(|| {
                anyhow!("QL_CONFIG_DIR is not set; the launcher sets it when it starts a plugin")
            })?,
            xdg_config_home: var("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")),
            agent_dir: var("PI_CODING_AGENT_DIR"),
            session_dir: var("PI_CODING_AGENT_SESSION_DIR"),
            home,
        })
    }

    /// The pins file this plugin writes.
    pub fn pins_file(&self) -> PathBuf {
        self.config_dir.join("pinned.json")
    }

    /// v0's pins file, read until [`pins_file`](Env::pins_file) exists and
    /// never written.
    pub fn v0_pins_file(&self) -> PathBuf {
        self.xdg_config_home.join("quick-launcher/pi-pinned.json")
    }

    /// Pins saved by the pi-launcher bar widget, read after v0's.
    pub fn widget_pins_file(&self) -> PathBuf {
        self.xdg_config_home.join("pi-launcher/pinned.json")
    }

    /// pi's session folder, resolved the way pi resolves it.
    pub fn sessions_dir(&self) -> PathBuf {
        if let Some(dir) = &self.session_dir {
            return expand_tilde(dir, &self.home);
        }
        let agent = expand_tilde(
            &self
                .agent_dir
                .clone()
                .unwrap_or_else(|| self.home.join(".pi/agent")),
            &self.home,
        );
        let configured = fs::read_to_string(agent.join("settings.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|settings| settings.get("sessionDir")?.as_str().map(String::from));
        if let Some(configured) = configured {
            let dir = expand_tilde(Path::new(&configured), &self.home);
            if dir.is_absolute() {
                return dir;
            }
            // pi resolves a relative sessionDir from each session's own
            // working directory, so there is no single folder to read
            // recents from.
            eprintln!(
                "ignoring relative sessionDir {configured:?}; \
                 recent projects come from the default session folder"
            );
        }
        agent.join("sessions")
    }
}

/// `~` and `~/…` against `home`. `~user` is left as it is.
fn expand_tilde(path: &Path, home: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

/// Pinned folders, in pin order. Reads the plugin's own file, else v0's,
/// else the widget's; none existing means no pins. A file that exists but
/// can't be read, isn't JSON or isn't a JSON object is an error, and the
/// files after it aren't tried. An object without a `projects` list is no
/// pins.
pub fn read_pins(env: &Env) -> anyhow::Result<Vec<String>> {
    for file in [env.pins_file(), env.v0_pins_file(), env.widget_pins_file()] {
        match fs::read_to_string(&file) {
            Ok(text) => {
                return parse_pins(&text)
                    .with_context(|| format!("{} is not a valid pins file", file.display()));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(anyhow!(e).context(format!("reading {}", file.display()))),
        }
    }
    Ok(Vec::new())
}

/// `{"projects": [...]}`. A missing or non-list `projects` is no pins, and
/// entries that aren't non-empty strings are skipped, as in v0.
fn parse_pins(text: &str) -> anyhow::Result<Vec<String>> {
    let Value::Object(file) = serde_json::from_str(text)? else {
        bail!("expected a JSON object");
    };
    let Some(Value::Array(projects)) = file.get("projects") else {
        return Ok(Vec::new());
    };
    Ok(projects
        .iter()
        .filter_map(|path| path.as_str())
        .filter(|path| !path.is_empty())
        .map(String::from)
        .collect())
}

/// Replaces the plugin's pins file, atomically. Only ever this plugin's own
/// file, so v0's stays as it was.
pub fn write_pins(env: &Env, pins: &[String]) -> anyhow::Result<()> {
    let file = env.pins_file();
    fs::create_dir_all(&env.config_dir)
        .with_context(|| format!("creating {}", env.config_dir.display()))?;
    let temporary = env
        .config_dir
        .join(format!(".pinned-{}.json", std::process::id()));
    let mut text = serde_json::to_string_pretty(&json!({ "projects": pins }))?;
    text.push('\n');
    let written = fs::write(&temporary, text).and_then(|()| fs::rename(&temporary, &file));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written.with_context(|| format!("writing {}", file.display()))
}

/// Pins `path` last, or unpins it. Pinning a pinned folder moves it to the
/// end, as v0 does. The first change carries the v0 or widget pins forward.
pub fn set_pinned(env: &Env, path: &str, pinned: bool) -> anyhow::Result<()> {
    let mut pins = read_pins(env)?;
    pins.retain(|pin| pin != path);
    if pinned {
        pins.push(path.to_owned());
    }
    write_pins(env, &pins)
}

/// Existing project folders from pi's sessions, most recently used first,
/// at most `limit`.
///
/// pi groups sessions per working directory, and each session file starts
/// with a header holding its `cwd`. A group's newest session with a readable
/// header counts. The home and root folders are not projects.
pub fn recent_projects(sessions: &Path, home: &Path, limit: usize) -> Vec<String> {
    let Ok(entries) = fs::read_dir(sessions) else {
        return Vec::new();
    };
    let entries: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    let mut groups: Vec<Vec<PathBuf>> = entries
        .iter()
        .filter(|path| is_session_file(path))
        .map(|path| vec![path.clone()])
        .collect();
    for dir in entries.iter().filter(|path| path.is_dir()) {
        if let Ok(files) = fs::read_dir(dir) {
            groups.push(
                files
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| is_session_file(path))
                    .collect(),
            );
        }
    }
    let home = home.to_string_lossy();
    let mut latest: HashMap<String, SystemTime> = HashMap::new();
    for files in groups {
        let Some((mtime, cwd)) = newest_session(files) else {
            continue;
        };
        if cwd == home || cwd == "/" {
            continue;
        }
        if latest.get(&cwd).is_none_or(|seen| mtime > *seen) && Path::new(&cwd).is_dir() {
            latest.insert(cwd, mtime);
        }
    }
    let mut recent: Vec<(String, SystemTime)> = latest.into_iter().collect();
    recent.sort_by(|(a, a_time), (b, b_time)| b_time.cmp(a_time).then_with(|| a.cmp(b)));
    recent.truncate(limit);
    recent.into_iter().map(|(cwd, _)| cwd).collect()
}

fn is_session_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "jsonl")
}

/// The modification time and `cwd` of the newest session file whose first
/// line is a header with a string `cwd`.
fn newest_session(files: Vec<PathBuf>) -> Option<(SystemTime, String)> {
    let mut stamped: Vec<(SystemTime, PathBuf)> = files
        .into_iter()
        .filter_map(|file| Some((fs::metadata(&file).ok()?.modified().ok()?, file)))
        .collect();
    stamped.sort_by(|a, b| b.cmp(a));
    stamped
        .into_iter()
        .find_map(|(mtime, file)| Some((mtime, session_cwd(&file)?)))
}

fn session_cwd(file: &Path) -> Option<String> {
    let mut line = String::new();
    BufReader::new(fs::File::open(file).ok()?)
        .read_line(&mut line)
        .ok()?;
    let header: Value = serde_json::from_str(&line).ok()?;
    header.get("cwd")?.as_str().map(String::from)
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

/// The items to list: pinned folders in pin order, then recent ones not
/// pinned. Each opens pi in a terminal in the folder, and pins or unpins it
/// as its second action.
///
/// As in v0, notes follow the path in `detail` rather than going in a
/// `badge`: `missing` for a pinned folder that's gone, which is also
/// disabled, and `last used` for the most recent project.
pub fn items(pinned: &[String], recent: &[String], home: &Path) -> Vec<Item> {
    let last_used = recent.first();
    let rows = pinned.iter().map(|path| (path, true)).chain(
        recent
            .iter()
            .filter(|path| !pinned.contains(path))
            .map(|path| (path, false)),
    );
    rows.map(|(path, is_pinned)| {
        let missing = is_pinned && !Path::new(path).is_dir();
        let mut detail = display_path(path, home);
        if missing {
            detail.push_str(" · missing");
        } else if Some(path) == last_used {
            detail.push_str(" · last used");
        }
        let mut item = Item::new(path, project_name(path))
            .detail(detail)
            .disabled(missing);
        if is_pinned {
            item = item.icon(PINNED_ICON);
        }
        let (id, title) = if is_pinned {
            ("unpin", "Unpin")
        } else {
            ("pin", "Pin")
        };
        item.action(Action::new("open", "Open pi").run(Run::terminal(["pi"]).cwd(path)))
            .action(Action::new(id, title).after(After::Refresh))
    })
    .collect()
}

/// The plugin: lists [`items`] for its [`Env`] and runs pin and unpin.
pub struct Pi(pub Env);

impl Plugin for Pi {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        let env = &self.0;
        // A broken pins file still lists the recent projects, as in v0.
        let pinned = read_pins(env).unwrap_or_else(|e| {
            eprintln!("ignoring pins: {e:#}");
            Vec::new()
        });
        let recent = recent_projects(&env.sessions_dir(), &env.home, RECENT_LIMIT);
        Ok(items(&pinned, &recent, &env.home))
    }

    fn activate(&mut self, activate: Activate) -> anyhow::Result<Outcome> {
        let path = activate
            .item
            .ok_or_else(|| anyhow!("pi has no query item"))?;
        match activate.action.as_str() {
            "pin" => set_pinned(&self.0, &path, true)?,
            "unpin" => set_pinned(&self.0, &path, false)?,
            other => bail!("unknown action {other:?}"),
        }
        Ok(Outcome::refresh())
    }
}
