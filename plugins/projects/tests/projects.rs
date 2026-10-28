//! The plugin over temporary folders: saving, searching and opening
//! projects, and completing typed paths as `cd` does.

use std::fs;
use std::io::{self, BufReader};
use std::path::PathBuf;
use std::thread;

use quick_launcher_plugin::run_on;
use quick_launcher_projects::{
    Env, Projects, SHELL, completions, display_path, expand, normalize, read_projects,
};
use quick_launcher_wire::framing;
use serde_json::{Value, json};

/// A home folder and a state folder under a fresh temporary folder,
/// removed on drop.
struct Dirs {
    root: PathBuf,
}

impl Dirs {
    fn new(test: &str) -> Dirs {
        let root = std::env::temp_dir().join(format!("ql-projects-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).unwrap();
        Dirs { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    /// Creates folders under home.
    fn mkdirs(&self, paths: &[&str]) {
        for path in paths {
            fs::create_dir_all(self.home().join(path)).unwrap();
        }
    }

    fn path(&self, rel: &str) -> String {
        self.home().join(rel).to_string_lossy().into_owned()
    }

    fn env(&self) -> Env {
        Env {
            state_dir: self.root.join("state"),
            home: self.home(),
        }
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Runs the plugin over one request and returns its answer.
fn answer(env: &Env, request: Value) -> Value {
    let input = format!("{request}\n").into_bytes();
    let (output, writer) = io::pipe().unwrap();
    let plugin = Projects(env.clone());
    let thread = thread::spawn(move || run_on(plugin, io::Cursor::new(input), writer));
    let mut output = BufReader::new(output);
    let mut buf = Vec::new();
    let response = framing::read_message(&mut output, &mut buf)
        .unwrap()
        .unwrap();
    thread.join().unwrap().unwrap();
    response
}

fn query(env: &Env, text: &str) -> Vec<Value> {
    let response = answer(
        env,
        json!({"v": 1, "id": 1, "method": "query", "params": {"text": text}}),
    );
    response["items"].as_array().unwrap().clone()
}

fn activate(env: &Env, item: &str, action: &str) -> Value {
    answer(
        env,
        json!({"v": 1, "id": 2, "method": "activate",
               "params": {"item": item, "action": action, "text": ""}}),
    )
}

fn ids(items: &[Value]) -> Vec<&str> {
    items
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect()
}

fn labels(items: &[Value]) -> Vec<&str> {
    items
        .iter()
        .map(|item| item["label"].as_str().unwrap())
        .collect()
}

#[test]
fn with_nothing_saved_only_the_add_row_is_listed() {
    let dirs = Dirs::new("empty");
    let items = query(&dirs.env(), "");
    assert_eq!(ids(&items), ["add"]);
    assert_eq!(items[0]["complete"], "~/");
    assert_eq!(
        items[0]["actions"][0]["run"],
        json!({"plugin": "start-add"})
    );
    let result = activate(&dirs.env(), "add", "start-add");
    assert_eq!(result["result"], json!({"close": false, "query": "~/"}));
}

#[test]
fn a_path_lists_the_folders_it_completes_to_like_cd() {
    let dirs = Dirs::new("complete");
    dirs.mkdirs(&["Projects/ql", "Programs", "pictures", ".config", "Public"]);
    fs::write(dirs.home().join("Projects.txt"), "not a folder").unwrap();
    let env = dirs.env();

    let items = query(&env, "~/Pro");
    assert_eq!(labels(&items), ["Programs/", "Projects/"]);
    assert_eq!(items[1]["complete"], "~/Projects/");
    assert_eq!(items[1]["detail"], "~/Projects");
    assert_eq!(items[1]["id"], format!("folder:{}", dirs.path("Projects")));
    let actions: Vec<&str> = items[1]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|action| action["id"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["add", "add-open"]);

    // Hidden folders only once a `.` is typed; case only when nothing
    // matches with it.
    assert_eq!(
        labels(&query(&env, "~/")),
        ["~", "pictures/", "Programs/", "Projects/", "Public/"]
    );
    assert_eq!(labels(&query(&env, "~/.c")), [".config/"]);
    assert_eq!(labels(&query(&env, "~/pi")), ["pictures/"]);
    assert_eq!(labels(&query(&env, "~/PI")), ["pictures/"]);
    assert_eq!(labels(&query(&env, "~/nope")), [] as [&str; 0]);

    // An absolute path completes in the same form.
    let absolute = format!("{}/Projects/", dirs.home().display());
    let items = query(&env, &absolute);
    assert_eq!(labels(&items), ["~/Projects", "ql/"]);
    assert_eq!(items[1]["complete"], format!("{absolute}ql/"));
}

#[test]
fn a_path_ending_in_a_slash_offers_the_folder_itself_without_a_completion() {
    let dirs = Dirs::new("itself");
    dirs.mkdirs(&["Projects/ql"]);
    let items = query(&dirs.env(), "~/Projects/");
    assert_eq!(labels(&items), ["~/Projects", "ql/"]);
    assert_eq!(items[0]["detail"], "Add this folder as a project");
    assert!(
        items[0].get("complete").is_none(),
        "Tab goes to the first folder"
    );
}

#[test]
fn adding_a_folder_saves_it_first_and_goes_back_to_the_list() {
    let dirs = Dirs::new("add");
    dirs.mkdirs(&["Projects/a", "Projects/b"]);
    let env = dirs.env();
    let (a, b) = (dirs.path("Projects/a"), dirs.path("Projects/b"));
    for path in [&a, &b] {
        let result = activate(&env, &format!("folder:{path}"), "add");
        assert_eq!(result["result"], json!({"close": false, "query": ""}));
    }
    assert_eq!(read_projects(&env).unwrap(), [b, a]);

    let items = query(&env, "");
    assert_eq!(labels(&items), ["b", "a", "Add a project folder…"]);
    assert_eq!(items[0]["detail"], "~/Projects/b");
    let actions = &items[0]["actions"];
    assert_eq!(actions[0]["id"], "open");
    assert_eq!(actions[1]["id"], "remove");
    assert_eq!(actions[1]["key"], "ctrl+d");
}

#[test]
fn search_matches_only_saved_projects_by_name_or_path() {
    let dirs = Dirs::new("search");
    dirs.mkdirs(&["Projects/quick-launcher", "Projects/notes", "Work/quiet"]);
    let env = dirs.env();
    for rel in ["Projects/quick-launcher", "Projects/notes"] {
        activate(&env, &format!("folder:{}", dirs.path(rel)), "add");
    }
    assert_eq!(
        labels(&query(&env, "qui")),
        ["quick-launcher", "Add a project folder…"],
        "Work/quiet isn't saved, so it isn't found"
    );
    assert_eq!(
        labels(&query(&env, "Projects/no")),
        ["notes", "Add a project folder…"]
    );
}

#[test]
fn a_path_lists_saved_projects_under_it_first_and_not_again_as_folders() {
    let dirs = Dirs::new("saved-path");
    dirs.mkdirs(&["Projects/ql", "Projects/qt"]);
    let env = dirs.env();
    activate(&env, &format!("folder:{}", dirs.path("Projects/qt")), "add");
    let items = query(&env, "~/Projects/q");
    assert_eq!(
        ids(&items),
        [
            format!("project:{}", dirs.path("Projects/qt")),
            format!("folder:{}", dirs.path("Projects/ql")),
        ]
    );
    assert_eq!(items[0]["complete"], "~/Projects/qt/");
    // A saved folder isn't offered for adding again.
    assert_eq!(
        labels(&query(&env, "~/Projects/")),
        ["qt", "~/Projects", "ql/"]
    );
}

#[test]
fn opening_a_project_moves_it_first_and_opens_a_terminal_there() {
    let dirs = Dirs::new("open");
    dirs.mkdirs(&["a", "b"]);
    let env = dirs.env();
    let (a, b) = (dirs.path("a"), dirs.path("b"));
    activate(&env, &format!("folder:{a}"), "add");
    activate(&env, &format!("folder:{b}"), "add");
    let result = activate(&env, &format!("project:{a}"), "open");
    assert_eq!(
        result["result"],
        json!({"close": true, "run": {"terminal": SHELL, "cwd": a}})
    );
    assert_eq!(read_projects(&env).unwrap(), [a.clone(), b.clone()]);

    // Adding and opening in one step.
    dirs.mkdirs(&["c"]);
    let c = dirs.path("c");
    let result = activate(&env, &format!("folder:{c}"), "add-open");
    assert_eq!(result["result"]["run"]["cwd"], c);
    assert_eq!(read_projects(&env).unwrap(), [c, a, b]);
}

#[test]
fn a_missing_project_is_marked_and_can_only_be_removed() {
    let dirs = Dirs::new("missing");
    dirs.mkdirs(&["gone"]);
    let env = dirs.env();
    let gone = dirs.path("gone");
    activate(&env, &format!("folder:{gone}"), "add");
    fs::remove_dir(&gone).unwrap();
    let items = query(&env, "");
    assert_eq!(items[0]["badge"], "missing");
    assert_eq!(items[0]["disabled"], true);
    let error = activate(&env, &format!("project:{gone}"), "open");
    assert_eq!(error["error"]["message"], "~/gone is not a folder");
    let result = activate(&env, &format!("project:{gone}"), "remove");
    assert_eq!(result["result"], json!({"close": false, "refresh": true}));
    assert_eq!(read_projects(&env).unwrap(), [] as [&str; 0]);
}

#[test]
fn a_folder_that_is_not_there_is_not_added() {
    let dirs = Dirs::new("not-there");
    let env = dirs.env();
    let error = activate(&env, &format!("folder:{}", dirs.path("nope")), "add");
    assert_eq!(error["error"]["message"], "~/nope is not a folder");
    assert!(read_projects(&env).unwrap().is_empty());
}

#[test]
fn a_broken_projects_file_still_completes_folders() {
    let dirs = Dirs::new("broken");
    dirs.mkdirs(&["Projects"]);
    let env = dirs.env();
    fs::create_dir_all(&env.state_dir).unwrap();
    fs::write(env.projects_file(), "not json").unwrap();
    assert_eq!(ids(&query(&env, "")), ["add"]);
    assert_eq!(labels(&query(&env, "~/P")), ["Projects/"]);
}

#[test]
fn paths_expand_and_normalize_as_cd_reads_them() {
    let home = PathBuf::from("/home/me");
    assert_eq!(expand("~", &home), Some(home.clone()));
    assert_eq!(expand("~/a/b", &home), Some(home.join("a/b")));
    assert_eq!(expand("/etc", &home), Some(PathBuf::from("/etc")));
    assert_eq!(expand("~other/x", &home), None);
    assert_eq!(expand("rel/x", &home), None);
    assert_eq!(expand("~/x", &PathBuf::new()), None);
    assert_eq!(normalize(&PathBuf::from("/a/./b/../c/")), "/a/c");
    assert_eq!(normalize(&PathBuf::from("/..")), "/");
    assert_eq!(display_path("/home/me/x", &home), "~/x");
    assert_eq!(display_path("/home/meet", &home), "/home/meet");
    assert!(completions("~user/", &home).is_empty());
    assert!(completions("no-slash", &home).is_empty());
}
