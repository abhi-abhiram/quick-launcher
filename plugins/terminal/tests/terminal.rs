//! The plugin over temporary folders: `tests/test_terminal_mode.py`'s
//! cases, the history migration, and the built binary answering requests.

use std::fs;
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use quick_launcher_plugin::run_on;
use quick_launcher_terminal::{Env, HISTORY_LIMIT, SCRIPT, Terminal, read_history};
use quick_launcher_wire::framing;
use serde_json::{Value, json};

/// State folders under a fresh temporary folder, removed on drop.
struct Dirs {
    root: PathBuf,
}

impl Dirs {
    fn new(test: &str) -> Dirs {
        let root = std::env::temp_dir().join(format!("ql-terminal-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Dirs { root }
    }

    fn env(&self) -> Env {
        Env {
            state_dir: self.root.join("state/quick-launcher/terminal"),
            xdg_state_home: self.root.join("state"),
        }
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn write_json(file: &Path, value: Value) {
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, value.to_string()).unwrap();
}

fn read_json(file: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap()
}

/// Runs the plugin over one request and returns its answer.
fn answer(env: &Env, request: Value) -> Value {
    let input = format!("{request}\n").into_bytes();
    let (output, writer) = io::pipe().unwrap();
    let plugin = Terminal(env.clone());
    let thread = thread::spawn(move || run_on(plugin, io::Cursor::new(input), writer));
    let mut output = BufReader::new(output);
    let mut buf = Vec::new();
    let response = framing::read_message(&mut output, &mut buf)
        .unwrap()
        .unwrap();
    thread.join().unwrap().unwrap();
    response
}

fn list(env: &Env) -> Vec<Value> {
    let response = answer(
        env,
        json!({"v": 1, "id": 1, "method": "list", "params": {}}),
    );
    response["items"].as_array().unwrap().clone()
}

fn activate(env: &Env, item: Option<&str>, action: &str, text: &str) -> Value {
    answer(
        env,
        json!({"v": 1, "id": 2, "method": "activate",
               "params": {"item": item, "action": action, "text": text}}),
    )
}

/// What the result of running `command` asks the launcher to run.
fn terminal(command: &str) -> Value {
    json!({"terminal": ["bash", "-ic", SCRIPT, "bash", command]})
}

/// Runs `command` as the `[query_item]` does, checking the result.
fn run(env: &Env, command: &str) {
    let response = activate(env, None, "run", command);
    assert_eq!(
        response["result"],
        json!({"close": true, "run": terminal(command)}),
        "{response}"
    );
}

fn labels(items: &[Value]) -> Vec<&str> {
    items
        .iter()
        .map(|item| item["label"].as_str().unwrap())
        .collect()
}

#[test]
fn history_is_most_recent_first_without_duplicates() {
    let dirs = Dirs::new("order");
    let env = dirs.env();
    for command in ["ls", "htop", "ls"] {
        run(&env, command);
    }
    let items = list(&env);
    assert_eq!(labels(&items), ["ls", "htop"]);
    assert_eq!(
        items[0],
        json!({"id": "ls", "label": "ls", "detail": "Run again", "icon": "",
        "actions": [
            {"id": "run", "title": "Run", "run": {"plugin": "run"}},
            {"id": "forget", "title": "Forget", "run": {"plugin": "forget"}, "after": "refresh"},
        ]})
    );
}

#[test]
fn running_a_history_item_runs_it_again_and_moves_it_first() {
    let dirs = Dirs::new("again");
    let env = dirs.env();
    run(&env, "ls");
    run(&env, "htop");
    // The item's id is the command; the text is whatever was typed.
    let response = activate(&env, Some("ls"), "run", "l");
    assert_eq!(
        response["result"],
        json!({"close": true, "run": terminal("ls")})
    );
    assert_eq!(read_history(&env).unwrap(), ["ls", "htop"]);
}

#[test]
fn forget_removes_a_command() {
    let dirs = Dirs::new("forget");
    let env = dirs.env();
    run(&env, "ls");
    run(&env, "htop");
    let response = activate(&env, Some("ls"), "forget", "");
    assert_eq!(response["result"], json!({"close": false, "refresh": true}));
    assert_eq!(read_history(&env).unwrap(), ["htop"]);
}

#[test]
fn history_is_capped() {
    let dirs = Dirs::new("cap");
    let env = dirs.env();
    for index in 0..HISTORY_LIMIT + 5 {
        run(&env, &format!("echo {index}"));
    }
    let history = read_history(&env).unwrap();
    assert_eq!(history.len(), HISTORY_LIMIT);
    assert_eq!(history[0], format!("echo {}", HISTORY_LIMIT + 4));
}

#[test]
fn run_remembers_then_opens_a_terminal_that_stays_open() {
    let dirs = Dirs::new("run");
    let env = dirs.env();
    run(&env, "make test");
    assert_eq!(read_history(&env).unwrap(), ["make test"]);
    // The command is an argument, never part of the script.
    assert_eq!(SCRIPT, r#"eval "$1"; exec "${SHELL:-bash}""#);
}

#[test]
fn history_is_readable_by_its_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dirs = Dirs::new("mode");
    let env = dirs.env();
    run(&env, "export TOKEN=secret");
    let mode = |file: &Path| fs::metadata(file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&env.history_file()), 0o600);
    // Rewriting a file someone made readable doesn't keep that mode.
    fs::set_permissions(env.history_file(), fs::Permissions::from_mode(0o644)).unwrap();
    run(&env, "ls");
    assert_eq!(mode(&env.history_file()), 0o600);
}

#[test]
fn blank_command_is_rejected() {
    let dirs = Dirs::new("blank");
    let env = dirs.env();
    let response = activate(&env, None, "run", "  ");
    assert!(response["error"]["message"].is_string(), "{response}");
    assert_eq!(read_history(&env).unwrap(), Vec::<String>::new());
}

#[test]
fn an_unknown_action_is_an_error() {
    let dirs = Dirs::new("unknown");
    let response = activate(&dirs.env(), Some("ls"), "pin", "");
    assert!(response["error"]["message"].is_string(), "{response}");
}

#[test]
fn invalid_history_entries_are_skipped() {
    let dirs = Dirs::new("entries");
    let env = dirs.env();
    write_json(
        &env.history_file(),
        json!({"commands": ["ls", "", "  ", 3, null, "htop"]}),
    );
    assert_eq!(labels(&list(&env)), ["ls", "htop"]);
    write_json(&env.history_file(), json!({"commands": "ls"}));
    assert_eq!(list(&env), Vec::<Value>::new());
}

#[test]
fn a_broken_history_lists_nothing_still_runs_and_is_not_overwritten() {
    let dirs = Dirs::new("broken");
    let env = dirs.env();
    fs::create_dir_all(&env.state_dir).unwrap();
    fs::write(env.history_file(), "not json").unwrap();
    assert_eq!(list(&env), Vec::<Value>::new());
    run(&env, "ls");
    let response = activate(&env, Some("ls"), "forget", "");
    assert!(response["error"]["message"].is_string(), "{response}");
    assert_eq!(fs::read_to_string(env.history_file()).unwrap(), "not json");
}

#[test]
fn reads_the_v0_history_until_its_own_exists() {
    let dirs = Dirs::new("v0");
    let env = dirs.env();
    write_json(&env.v0_history_file(), json!({"commands": ["ls", "htop"]}));
    assert_eq!(labels(&list(&env)), ["ls", "htop"]);

    write_json(&env.history_file(), json!({"commands": ["make"]}));
    assert_eq!(labels(&list(&env)), ["make"]);
}

#[test]
fn the_first_write_carries_the_v0_history_forward_and_leaves_it_untouched() {
    let dirs = Dirs::new("carry");
    let env = dirs.env();
    let v0 = json!({"commands": ["ls", "htop"]});
    write_json(&env.v0_history_file(), v0.clone());

    run(&env, "make");
    assert_eq!(
        read_json(&env.history_file()),
        json!({"commands": ["make", "ls", "htop"]})
    );
    assert_eq!(read_json(&env.v0_history_file()), v0);

    // Forgetting a v0 command before anything was run carries the rest.
    let dirs = Dirs::new("carry-forget");
    let env = dirs.env();
    write_json(&env.v0_history_file(), v0.clone());
    activate(&env, Some("ls"), "forget", "");
    assert_eq!(
        read_json(&env.history_file()),
        json!({"commands": ["htop"]})
    );
    assert_eq!(read_json(&env.v0_history_file()), v0);
}

#[test]
fn the_binary_lists_and_runs_with_history_in_its_state_dir() {
    let dirs = Dirs::new("binary");
    let state = dirs.root.join("ql-state");

    let request = |request: Value| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_terminal-plugin"))
            .env("HOME", dirs.root.join("home"))
            .env("XDG_STATE_HOME", dirs.root.join("state"))
            .env("QL_STATE_DIR", &state)
            .env("QL_CONFIG_DIR", dirs.root.join("ql-config"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(format!("{request}\n").as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.len(), 1, "one response line: {stdout}");
        serde_json::from_str::<Value>(lines[0]).unwrap()
    };

    let ran = request(json!({"v": 1, "id": 1, "method": "activate",
                             "params": {"item": null, "action": "run", "text": "ls -la"}}));
    assert_eq!(
        ran["result"],
        json!({"close": true, "run": terminal("ls -la")})
    );
    assert_eq!(
        read_json(&state.join("history.json")),
        json!({"commands": ["ls -la"]})
    );
    let listed = request(json!({"v": 1, "id": 2, "method": "list", "params": {}}));
    assert_eq!(listed["items"][0]["label"], "ls -la");
}

#[test]
fn the_binary_needs_its_state_dir() {
    let output = Command::new(env!("CARGO_BIN_EXE_terminal-plugin"))
        .env_remove("QL_STATE_DIR")
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("QL_STATE_DIR"));
}
