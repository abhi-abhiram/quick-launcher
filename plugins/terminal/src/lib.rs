//! The bundled `terminal` plugin (*Examples* in `docs/protocol.md`): run a
//! shell command in a new terminal and remember it. A port of v0's
//! `modes/terminal.py`.
//!
//! History items run their command again, and forget it as their second
//! action. The typed command is the manifest's `[query_item]`. Running
//! either is a `plugin` action, so the command moves to the top of the
//! history, and its result has the launcher open the terminal.
//!
//! The binary reads its settings from the environment ([`Env::from_process`]).
//! Tests build an [`Env`] by hand.

use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

use anyhow::{Context, anyhow, bail};
use quick_launcher_plugin::{Action, Activate, After, Item, Outcome, Plugin, Run};
use serde_json::{Value, json};

/// How many commands the history keeps, as in v0.
pub const HISTORY_LIMIT: usize = 100;

/// The history item icon, as in v0.
pub const ICON: &str = "";

/// What a new terminal runs: an interactive bash, so the user's aliases
/// expand, that hands over to their shell once the command exits, so its
/// output stays readable. The command is `$1`, never part of the script.
pub const SCRIPT: &str = r#"eval "$1"; exec "${SHELL:-bash}""#;

/// Where the plugin reads and writes.
#[derive(Debug, Clone)]
pub struct Env {
    /// `QL_STATE_DIR`, where history is written. Created on the first write.
    pub state_dir: PathBuf,
    /// `XDG_STATE_HOME`, else `~/.local/state`: where v0 kept history.
    pub xdg_state_home: PathBuf,
}

impl Env {
    /// The process environment. Fails when `QL_STATE_DIR` isn't set, which
    /// the core always sets. The plugin keeps nothing in `QL_CONFIG_DIR`,
    /// so it isn't needed.
    pub fn from_process() -> anyhow::Result<Env> {
        let var = |name: &str| {
            env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let home = var("HOME").unwrap_or_default();
        Ok(Env {
            state_dir: var("QL_STATE_DIR").ok_or_else(|| {
                anyhow!("QL_STATE_DIR is not set; the launcher sets it when it starts a plugin")
            })?,
            xdg_state_home: var("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state")),
        })
    }

    /// The history file this plugin writes.
    pub fn history_file(&self) -> PathBuf {
        self.state_dir.join("history.json")
    }

    /// v0's history file, read until [`history_file`](Env::history_file)
    /// exists and never written.
    pub fn v0_history_file(&self) -> PathBuf {
        self.xdg_state_home
            .join("quick-launcher/terminal-history.json")
    }
}

/// Remembered commands, most recent first. Reads the plugin's own file,
/// else v0's; neither existing means no history. A file that exists but
/// can't be read, isn't JSON or isn't a JSON object is an error, and v0's
/// isn't tried after the plugin's own. An object without a `commands` list
/// is no history.
pub fn read_history(env: &Env) -> anyhow::Result<Vec<String>> {
    for file in [env.history_file(), env.v0_history_file()] {
        match fs::read_to_string(&file) {
            Ok(text) => {
                return parse_history(&text)
                    .with_context(|| format!("{} is not a valid history file", file.display()));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(anyhow!(e).context(format!("reading {}", file.display()))),
        }
    }
    Ok(Vec::new())
}

/// `{"commands": [...]}`. A missing or non-list `commands` is no history,
/// and entries that aren't strings with something besides whitespace are
/// skipped, as in v0.
fn parse_history(text: &str) -> anyhow::Result<Vec<String>> {
    let Value::Object(file) = serde_json::from_str(text)? else {
        bail!("expected a JSON object");
    };
    let Some(Value::Array(commands)) = file.get("commands") else {
        return Ok(Vec::new());
    };
    Ok(commands
        .iter()
        .filter_map(|command| command.as_str())
        .filter(|command| !command.trim().is_empty())
        .map(String::from)
        .collect())
}

/// Replaces the plugin's history file with the first [`HISTORY_LIMIT`]
/// commands, atomically. Only ever this plugin's own file, so v0's stays as
/// it was.
pub fn write_history(env: &Env, commands: &[String]) -> anyhow::Result<()> {
    let file = env.history_file();
    fs::create_dir_all(&env.state_dir)
        .with_context(|| format!("creating {}", env.state_dir.display()))?;
    let temporary = env
        .state_dir
        .join(format!(".history-{}.json", std::process::id()));
    let kept = &commands[..commands.len().min(HISTORY_LIMIT)];
    let mut text = serde_json::to_string_pretty(&json!({ "commands": kept }))?;
    text.push('\n');
    let written = fs::write(&temporary, text).and_then(|()| fs::rename(&temporary, &file));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written.with_context(|| format!("writing {}", file.display()))
}

/// Puts `command` first, once. The first change carries v0's history
/// forward.
pub fn remember(env: &Env, command: &str) -> anyhow::Result<()> {
    let mut commands = read_history(env)?;
    commands.retain(|item| item != command);
    commands.insert(0, command.to_owned());
    write_history(env, &commands)
}

/// Removes `command` from the history.
pub fn forget(env: &Env, command: &str) -> anyhow::Result<()> {
    let mut commands = read_history(env)?;
    commands.retain(|item| item != command);
    write_history(env, &commands)
}

/// Opens `command` in a new terminal, through [`SCRIPT`]. Its folder is
/// the launcher's default, the home folder.
pub fn terminal_run(command: &str) -> Run {
    Run::terminal(["bash", "-ic", SCRIPT, "bash", command])
}

/// The items to list: one per remembered command, in history order. Each
/// runs its command again, and forgets it as its second action.
pub fn items(commands: &[String]) -> Vec<Item> {
    commands
        .iter()
        .map(|command| {
            Item::new(command, command)
                .detail("Run again")
                .icon(ICON)
                .action(Action::new("run", "Run"))
                .action(Action::new("forget", "Forget").after(After::Refresh))
        })
        .collect()
}

/// The plugin: lists [`items`] for its [`Env`], runs commands and forgets
/// them.
pub struct Terminal(pub Env);

impl Plugin for Terminal {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        // A broken history file lists nothing, as in v0.
        Ok(items(&read_history(&self.0).unwrap_or_else(|e| {
            eprintln!("ignoring history: {e:#}");
            Vec::new()
        })))
    }

    fn activate(&mut self, activate: Activate) -> anyhow::Result<Outcome> {
        // A history item's id is its command; the `[query_item]` has none
        // and sends the typed text.
        let command = activate.item.unwrap_or(activate.text);
        match activate.action.as_str() {
            "run" => {
                if command.trim().is_empty() {
                    bail!("there is no command to run");
                }
                // The command still runs when it can't be remembered.
                if let Err(e) = remember(&self.0, &command) {
                    eprintln!("not remembering {command:?}: {e:#}");
                }
                Ok(Outcome::close().run(terminal_run(&command)))
            }
            "forget" => {
                forget(&self.0, &command)?;
                Ok(Outcome::refresh())
            }
            other => bail!("unknown action {other:?}"),
        }
    }
}
