//! The plugin over temporary folders: v0's `tests/test_pi_mode.py` cases, the
//! pins migration, and the built binary answering a `list` request.

use std::fs;
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime};

use quick_launcher_pi::{Env, Pi, read_pins};
use quick_launcher_plugin::run_on;
use quick_launcher_wire::framing;
use serde_json::{Value, json};

/// A home, a session folder and config folders under a fresh temporary
/// folder, removed on drop.
struct Dirs {
    root: PathBuf,
}

impl Dirs {
    fn new(test: &str) -> Dirs {
        let root = std::env::temp_dir().join(format!("ql-pi-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).unwrap();
        fs::create_dir_all(root.join("sessions")).unwrap();
        Dirs { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn sessions(&self) -> PathBuf {
        self.root.join("sessions")
    }

    fn env(&self) -> Env {
        Env {
            config_dir: self.root.join("config/quick-launcher/pi"),
            xdg_config_home: self.root.join("config"),
            home: self.home(),
            agent_dir: None,
            session_dir: Some(self.sessions()),
        }
    }

    /// Makes `~/<name>` and returns its path.
    fn project(&self, name: &str) -> String {
        let path = self.home().join(name);
        fs::create_dir_all(&path).unwrap();
        path.to_str().unwrap().to_owned()
    }

    /// A session file in `group` whose header names `cwd`, modified at
    /// `mtime` seconds.
    fn session(&self, cwd: &str, mtime: u64, group: &str) {
        let folder = self.sessions().join(group);
        fs::create_dir_all(&folder).unwrap();
        let file = folder.join(format!("{mtime}.jsonl"));
        write_at(
            &file,
            &format!("{}\n", json!({"type": "session", "cwd": cwd})),
            mtime,
        );
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn write_at(file: &Path, text: &str, mtime: u64) {
    fs::write(file, text).unwrap();
    fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(mtime))
        .unwrap();
}

fn write_json(file: &Path, value: Value) {
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, value.to_string()).unwrap();
}

/// Runs the plugin over one request and returns its answer.
fn answer(env: Env, request: Value) -> Value {
    let input = format!("{request}\n").into_bytes();
    let (output, writer) = io::pipe().unwrap();
    let thread = thread::spawn(move || run_on(Pi(env), io::Cursor::new(input), writer));
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
        env.clone(),
        json!({"v": 1, "id": 1, "method": "list", "params": {}}),
    );
    response["items"].as_array().unwrap().clone()
}

fn activate(env: &Env, item: Option<&str>, action: &str) -> Value {
    answer(
        env.clone(),
        json!({"v": 1, "id": 2, "method": "activate",
               "params": {"item": item, "action": action, "text": ""}}),
    )
}

fn pin(env: &Env, path: &str) {
    let response = activate(env, Some(path), "pin");
    assert_eq!(
        response["result"],
        json!({"close": false, "refresh": true}),
        "{response}"
    );
}

fn unpin(env: &Env, path: &str) {
    let response = activate(env, Some(path), "unpin");
    assert_eq!(
        response["result"],
        json!({"close": false, "refresh": true}),
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
fn lists_pinned_then_recent_projects() {
    let dirs = Dirs::new("order");
    let env = dirs.env();
    let older = dirs.project("older");
    let newer = dirs.project("newer");
    let pinned = dirs.project("pinned");
    dirs.session(&older, 100, "a");
    dirs.session(&newer, 200, "b");
    dirs.session(dirs.home().to_str().unwrap(), 300, "c");
    pin(&env, &pinned);

    let items = list(&env);

    assert_eq!(labels(&items), ["pinned", "newer", "older"]);
    assert_eq!(
        items[0],
        json!({"id": pinned, "label": "pinned", "detail": "~/pinned", "icon": "󰐃",
        "actions": [
            {"id": "open", "title": "Open pi", "run": {"terminal": ["pi"], "cwd": pinned}},
            {"id": "unpin", "title": "Unpin", "run": {"plugin": "unpin"}, "after": "refresh"},
        ]})
    );
    assert_eq!(
        items[1],
        json!({"id": newer, "label": "newer", "detail": "~/newer · last used",
        "actions": [
            {"id": "open", "title": "Open pi", "run": {"terminal": ["pi"], "cwd": newer}},
            {"id": "pin", "title": "Pin", "run": {"plugin": "pin"}, "after": "refresh"},
        ]})
    );
    assert_eq!(items[2]["detail"], "~/older");
}

#[test]
fn pinned_recent_project_is_listed_once() {
    let dirs = Dirs::new("once");
    let env = dirs.env();
    let path = dirs.project("both");
    dirs.session(&path, 100, "group");
    pin(&env, &path);
    let items = list(&env);
    assert_eq!(labels(&items), ["both"]);
    assert_eq!(items[0]["detail"], "~/both · last used");
}

#[test]
fn missing_pinned_folder_is_disabled_but_unpinnable() {
    let dirs = Dirs::new("missing");
    let env = dirs.env();
    let gone = dirs.home().join("gone").to_str().unwrap().to_owned();
    pin(&env, &gone);

    let [item] = &list(&env)[..] else {
        panic!("one item")
    };
    assert_eq!(item["disabled"], true);
    assert_eq!(item["detail"], "~/gone · missing");
    assert_eq!(item["actions"][1]["title"], "Unpin");

    unpin(&env, &gone);
    assert_eq!(list(&env), Vec::<Value>::new());
}

#[test]
fn unpin_removes_and_pin_does_not_duplicate() {
    let dirs = Dirs::new("toggle");
    let env = dirs.env();
    let a = dirs.project("a");
    let b = dirs.project("b");
    pin(&env, &a);
    pin(&env, &b);
    pin(&env, &a);
    assert_eq!(read_pins(&env).unwrap(), [b.clone(), a.clone()]);
    unpin(&env, &a);
    assert_eq!(read_pins(&env).unwrap(), [b]);
}

#[test]
fn pins_are_written_to_the_config_dir() {
    let dirs = Dirs::new("written");
    let env = dirs.env();
    let path = dirs.project("project");
    pin(&env, &path);
    let text = fs::read_to_string(env.config_dir.join("pinned.json")).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        json!({"projects": [path]})
    );
    assert!(text.ends_with('\n'));
    assert_eq!(fs::read_dir(&env.config_dir).unwrap().count(), 1);
}

#[test]
fn reads_v0_pins_until_the_first_change_carries_them_forward() {
    let dirs = Dirs::new("v0");
    let env = dirs.env();
    let kept = dirs.project("kept");
    let added = dirs.project("added");
    let v0 = env.v0_pins_file();
    write_json(&v0, json!({"projects": [kept]}));
    let v0_text = fs::read_to_string(&v0).unwrap();

    assert_eq!(labels(&list(&env)), ["kept"]);
    pin(&env, &added);

    assert_eq!(read_pins(&env).unwrap(), [kept.clone(), added.clone()]);
    assert!(env.pins_file().exists());
    assert_eq!(
        fs::read_to_string(&v0).unwrap(),
        v0_text,
        "v0 file untouched"
    );

    // From now on the plugin's own file is the one read.
    unpin(&env, &kept);
    assert_eq!(labels(&list(&env)), ["added"]);
    assert_eq!(
        fs::read_to_string(&v0).unwrap(),
        v0_text,
        "v0 file untouched"
    );
}

#[test]
fn reads_pins_from_the_old_bar_widget_until_changed() {
    let dirs = Dirs::new("widget");
    let env = dirs.env();
    let kept = dirs.project("kept");
    let added = dirs.project("added");
    let widget = env.widget_pins_file();
    write_json(&widget, json!({"projects": [kept]}));

    assert_eq!(read_pins(&env).unwrap(), std::slice::from_ref(&kept));
    pin(&env, &added);

    assert_eq!(read_pins(&env).unwrap(), [kept.clone(), added]);
    assert_eq!(
        fs::read_to_string(&widget).unwrap(),
        json!({"projects": [kept]}).to_string()
    );
}

#[test]
fn v0_pins_come_before_the_widget_pins() {
    let dirs = Dirs::new("v0-first");
    let env = dirs.env();
    let v0 = dirs.project("v0");
    let widget = dirs.project("widget");
    write_json(&env.v0_pins_file(), json!({"projects": [v0]}));
    write_json(&env.widget_pins_file(), json!({"projects": [widget]}));
    assert_eq!(read_pins(&env).unwrap(), [v0]);
}

#[test]
fn pins_that_are_not_non_empty_strings_are_skipped() {
    let dirs = Dirs::new("entries");
    let env = dirs.env();
    let path = dirs.project("project");
    write_json(&env.pins_file(), json!({"projects": [1, "", null, path]}));
    assert_eq!(read_pins(&env).unwrap(), [path]);
    write_json(&env.pins_file(), json!({"projects": "x"}));
    assert_eq!(read_pins(&env).unwrap(), Vec::<String>::new());
}

#[test]
fn invalid_pin_file_lists_no_pins_and_is_not_overwritten() {
    let dirs = Dirs::new("invalid");
    let env = dirs.env();
    let recent = dirs.project("recent");
    dirs.session(&recent, 100, "group");
    fs::create_dir_all(&env.config_dir).unwrap();
    fs::write(env.pins_file(), "{").unwrap();

    assert!(read_pins(&env).is_err());
    assert_eq!(labels(&list(&env)), ["recent"]);

    let response = activate(&env, Some(&recent), "pin");
    let message = response["error"]["message"].as_str().unwrap();
    assert!(message.contains("pinned.json"), "{response}");
    assert_eq!(fs::read_to_string(env.pins_file()).unwrap(), "{");
}

#[test]
fn activate_needs_an_item_and_a_known_action() {
    let dirs = Dirs::new("activate");
    let env = dirs.env();
    let path = dirs.project("project");
    assert!(activate(&env, None, "pin")["error"].is_object());
    assert!(activate(&env, Some(&path), "open")["error"].is_object());
    assert!(!env.pins_file().exists());
}

#[test]
fn relative_session_dir_falls_back_to_default() {
    let dirs = Dirs::new("relative");
    let agent = dirs.root.join("agent");
    write_json(&agent.join("settings.json"), json!({"sessionDir": "rel"}));
    let env = Env {
        agent_dir: Some(agent.clone()),
        session_dir: None,
        ..dirs.env()
    };
    assert_eq!(env.sessions_dir(), agent.join("sessions"));
}

#[test]
fn session_dir_comes_from_the_variable_then_settings_then_the_default() {
    let dirs = Dirs::new("sessions-dir");
    let agent = dirs.root.join("agent");
    let env = Env {
        agent_dir: Some(agent.clone()),
        session_dir: None,
        ..dirs.env()
    };
    assert_eq!(env.sessions_dir(), agent.join("sessions"));

    write_json(
        &agent.join("settings.json"),
        json!({"sessionDir": "~/pi-sessions"}),
    );
    assert_eq!(env.sessions_dir(), dirs.home().join("pi-sessions"));

    let env = Env {
        session_dir: Some(PathBuf::from("~/elsewhere")),
        ..env
    };
    assert_eq!(env.sessions_dir(), dirs.home().join("elsewhere"));

    let env = Env {
        agent_dir: None,
        session_dir: None,
        ..env
    };
    assert_eq!(env.sessions_dir(), dirs.home().join(".pi/agent/sessions"));
}

#[test]
fn unreadable_newest_session_falls_back_to_older() {
    let dirs = Dirs::new("unreadable");
    let env = dirs.env();
    let path = dirs.project("project");
    dirs.session(&path, 100, "group");
    write_at(&dirs.sessions().join("group/200.jsonl"), "not json\n", 200);
    assert_eq!(labels(&list(&env)), ["project"]);
}

#[test]
fn a_group_counts_its_newest_session_only() {
    let dirs = Dirs::new("newest");
    let env = dirs.env();
    let moved = dirs.project("moved");
    let other = dirs.project("other");
    // The group's newest session was in `other`, so `moved`'s older session
    // in that group doesn't count.
    dirs.session(&moved, 300, "group");
    dirs.session(&other, 400, "group");
    assert_eq!(labels(&list(&env)), ["other"]);
}

#[test]
fn top_level_session_files_count_and_root_and_missing_folders_do_not() {
    let dirs = Dirs::new("top-level");
    let env = dirs.env();
    let loose = dirs.project("loose");
    let grouped = dirs.project("grouped");
    write_at(
        &dirs.sessions().join("loose.jsonl"),
        &format!("{}\n", json!({"cwd": loose})),
        200,
    );
    dirs.session(&grouped, 100, "g1");
    dirs.session("/", 500, "g2");
    dirs.session(dirs.home().join("gone").to_str().unwrap(), 600, "g3");
    assert_eq!(labels(&list(&env)), ["loose", "grouped"]);
}

#[test]
fn recent_projects_are_limited_to_twenty() {
    let dirs = Dirs::new("limit");
    let env = dirs.env();
    for n in 0..25 {
        let path = dirs.project(&format!("p{n:02}"));
        dirs.session(&path, 100 + n, &format!("g{n}"));
    }
    let items = list(&env);
    assert_eq!(items.len(), 20);
    assert_eq!(items[0]["label"], "p24");
    assert_eq!(items[19]["label"], "p05");
}

#[test]
fn paths_outside_home_keep_their_full_path() {
    let dirs = Dirs::new("outside");
    let env = dirs.env();
    let outside = dirs.root.join("elsewhere");
    fs::create_dir_all(&outside).unwrap();
    let outside = outside.to_str().unwrap();
    // A sibling whose name starts with the home folder's isn't under it.
    let sibling = format!("{}2", dirs.home().display());
    fs::create_dir_all(&sibling).unwrap();
    pin(&env, outside);
    pin(&env, &sibling);
    let items = list(&env);
    assert_eq!(items[0]["detail"], outside);
    assert_eq!(items[1]["detail"], sibling);
    assert_eq!(items[1]["label"], "home2");
}

#[test]
fn the_binary_lists_projects_and_pins_in_its_config_dir() {
    let dirs = Dirs::new("binary");
    let path = dirs.project("project");
    dirs.session(&path, 100, "group");
    let config = dirs.root.join("ql-config");

    let run = |request: Value| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_pi-plugin"))
            .env("HOME", dirs.home())
            .env("XDG_CONFIG_HOME", dirs.root.join("config"))
            .env("PI_CODING_AGENT_SESSION_DIR", dirs.sessions())
            .env("QL_CONFIG_DIR", &config)
            .env("QL_STATE_DIR", dirs.root.join("ql-state"))
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

    let listed = run(json!({"v": 1, "id": 1, "method": "list", "params": {}}));
    assert_eq!(listed["items"][0]["label"], "project");
    assert_eq!(listed["items"][0]["detail"], "~/project · last used");

    let pinned = run(json!({"v": 1, "id": 2, "method": "activate",
                            "params": {"item": path, "action": "pin", "text": ""}}));
    assert_eq!(pinned["result"], json!({"close": false, "refresh": true}));
    let file: Value =
        serde_json::from_str(&fs::read_to_string(config.join("pinned.json")).unwrap()).unwrap();
    assert_eq!(file, json!({"projects": [path]}));
}

#[test]
fn the_binary_needs_its_config_dir() {
    let output = Command::new(env!("CARGO_BIN_EXE_pi-plugin"))
        .env_remove("QL_CONFIG_DIR")
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("QL_CONFIG_DIR"));
}
