//! The `oneshot` plugin host against the fixture plugins in
//! `crates/core/fixtures`, which this test builds first.
//!
//! Each fixture is laid out as a real plugin folder in a temporary home and
//! found by discovery, so the host runs it exactly as the core will.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use std::{env, fs, process};

use quick_launcher_core::discovery::scan;
use quick_launcher_core::host::{Answer, Failure, Launch, oneshot};
use quick_launcher_core::log::Lines;
use quick_launcher_core::platform::Omarchy;
use quick_launcher_core::{Dirs, Log, Plugin};
use quick_launcher_wire::{ActivateParams, Item, Method, QueryParams, Request};

/// The folder holding the fixture binaries, built on first use.
///
/// `cargo test` from the workspace builds them already, but `cargo test -p
/// quick-launcher-core` doesn't, so the build runs either way; when they're
/// up to date it does nothing.
fn fixture_bins() -> &'static Path {
    static BINS: OnceLock<PathBuf> = OnceLock::new();
    BINS.get_or_init(|| {
        // This test runs from <target>/<profile>/deps/.
        let exe = env::current_exe().unwrap();
        let profile_dir = exe.parent().unwrap().parent().unwrap().to_path_buf();
        let target_dir = profile_dir.parent().unwrap();
        let mut build = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        build
            .args(["build", "--bins", "-p", "quick-launcher-core-fixtures"])
            .arg("--target-dir")
            .arg(target_dir);
        if profile_dir.file_name() == Some("release".as_ref()) {
            build.arg("--release");
        }
        let status = build.status().expect("cargo runs");
        assert!(status.success(), "building the fixtures failed");
        profile_dir
    })
}

/// A temporary home with one fixture plugin in it, removed on drop.
struct Fixture {
    root: PathBuf,
    dirs: Dirs,
    plugin: Plugin,
}

impl Fixture {
    /// The fixture binary `fixture-<name>` as plugin `fixture`, with
    /// `extra` added to its manifest.
    fn new(name: &str, extra: &str) -> Fixture {
        let bin = fixture_bins().join(format!("fixture-{name}"));
        Fixture::with_bin(
            &|dir| std::os::unix::fs::symlink(&bin, dir.join("bin")).unwrap(),
            extra,
        )
    }

    /// A plugin whose binary `make_bin` puts in its folder as `bin`.
    fn with_bin(make_bin: &dyn Fn(&Path), extra: &str) -> Fixture {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!("ql-host-{}-{n}", process::id()));
        let _ = fs::remove_dir_all(&root);
        let config = root.join("config/quick-launcher");
        let dirs = Dirs {
            bundled_plugins: root.join("data/quick-launcher/plugins"),
            user_plugins: config.join("plugins"),
            config_file: config.join("config.toml"),
            plugin_config: config,
            plugin_state: root.join("state/quick-launcher"),
            home: root.clone(),
        };
        let dir = dirs.bundled_plugins.join("fixture");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("plugin.toml"),
            format!(
                "protocol = 1\nid = \"fixture\"\nname = \"Fixture\"\n\
                 prefixes = [\"fx:\"]\nbin = \"bin\"\n{extra}\n"
            ),
        )
        .unwrap();
        make_bin(&dir);
        let mut found = scan(&dirs);
        assert_eq!(found.warnings, [], "the fixture plugin should load");
        let plugin = found.plugins.remove(0);
        Fixture { root, dirs, plugin }
    }

    /// A launch on the Omarchy platform, which the host uses only for its
    /// directories and its login command, and a handle on what it logs.
    fn launch(&self) -> (Launch, Lines) {
        let (log, lines) = Log::memory();
        let platform = Omarchy::with_dirs(self.dirs.clone());
        (Launch::new(&platform, &self.plugin, log), lines)
    }

    /// Every answer to `method`, and the log.
    async fn run(&self, method: Method) -> (Vec<Answer>, Vec<String>) {
        let (launch, lines) = self.launch();
        let mut answers = Vec::new();
        oneshot(&launch, &Request::new(7, method), |answer| {
            answers.push(answer)
        })
        .await;
        (answers, lines.get())
    }

    /// The pid the fixture wrote before hanging.
    fn pid(&self) -> u32 {
        let path = self.dirs.state_for("fixture").join("pid");
        fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
            .parse()
            .unwrap()
    }

    /// Asserts the process that wrote its pid is gone. A process the host
    /// killed but whose exit only tokio's background reaper collects may
    /// stay a zombie briefly; that's not running either.
    fn assert_gone(&self) {
        let stat = PathBuf::from(format!("/proc/{}/stat", self.pid()));
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match fs::read_to_string(&stat) {
                Err(_) => return,
                Ok(stat)
                    if stat
                        .rsplit(')')
                        .next()
                        .unwrap()
                        .trim_start()
                        .starts_with('Z') =>
                {
                    return;
                }
                Ok(_) if Instant::now() > deadline => panic!("the plugin is still running"),
                Ok(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn query(text: &str) -> Method {
    Method::Query(QueryParams { text: text.into() })
}

fn ids(items: &[Item]) -> Vec<&str> {
    items.iter().map(|item| item.id.as_str()).collect()
}

/// The ids in a single final `Items` answer.
fn final_ids(answers: &[Answer]) -> Vec<&str> {
    match answers {
        [Answer::Items { items, more: false }] => ids(items),
        other => panic!("expected one final list of items, got {other:?}"),
    }
}

fn the_failure(answers: &[Answer]) -> &Failure {
    match answers {
        [Answer::Failed(failure)] => failure,
        other => panic!("expected one failure, got {other:?}"),
    }
}

fn logged(log: &[String], text: &str) -> bool {
    log.iter().any(|line| line.contains(text))
}

#[tokio::test]
async fn lists_items_and_forwards_stderr() {
    let fixture = Fixture::new("list", "");
    let (answers, log) = fixture.run(Method::list()).await;
    assert_eq!(final_ids(&answers), ["one", "two"]);
    assert!(
        log.contains(&"fixture: listing two items".to_owned()),
        "{log:?}"
    );
}

#[tokio::test]
async fn activate_returns_the_result() {
    let fixture = Fixture::new("list", "");
    let method = Method::Activate(ActivateParams {
        item: Some("one".into()),
        action: "pin".into(),
        text: String::new(),
    });
    let (answers, log) = fixture.run(method).await;
    match &answers[..] {
        [Answer::Result(result)] => assert!(result.refresh && !result.closes()),
        other => panic!("expected a result, got {other:?}"),
    }
    assert!(
        logged(&log, r#"fixture: activating "pin" on Some("one")"#),
        "{log:?}"
    );
}

#[tokio::test]
async fn a_plugin_that_never_answers_is_killed() {
    let fixture = Fixture::new("slow", "timeout_ms = 300");
    let started = Instant::now();
    let (answers, log) = fixture.run(Method::list()).await;
    assert_eq!(
        the_failure(&answers),
        &Failure::NoResponse(Duration::from_millis(300))
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(logged(&log, "didn't respond within 300 ms"), "{log:?}");
    fixture.assert_gone();
}

#[tokio::test]
async fn a_slow_answer_within_the_timeout_is_used() {
    let fixture = Fixture::new("slow", "kind = \"query\"\ntimeout_ms = 2000");
    let (answers, _) = fixture.run(query("200")).await;
    assert_eq!(final_ids(&answers), ["late"]);
}

#[tokio::test]
async fn a_plugin_still_running_after_answering_is_killed() {
    let fixture = Fixture::new("slow", "kind = \"query\"\ntimeout_ms = 300");
    let (answers, log) = fixture.run(query("linger")).await;
    assert_eq!(final_ids(&answers), ["late"]);
    assert!(
        logged(&log, "still running 300 ms after answering"),
        "{log:?}"
    );
    fixture.assert_gone();
}

#[tokio::test]
async fn a_panic_is_an_error_answer() {
    let fixture = Fixture::new("panic", "");
    let (answers, _) = fixture.run(Method::list()).await;
    match the_failure(&answers) {
        Failure::Plugin(message) => assert!(message.contains("the list is on fire"), "{message}"),
        other => panic!("expected the plugin's error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_line_that_isnt_json_is_an_error() {
    let fixture = Fixture::new("garbage", "kind = \"query\"");
    let (answers, log) = fixture.run(query("garbage")).await;
    assert_eq!(the_failure(&answers), &Failure::Invalid);
    assert!(logged(&log, "fixture: unreadable response"), "{log:?}");
}

#[tokio::test]
async fn broken_items_are_dropped_and_the_rest_kept() {
    let fixture = Fixture::new("garbage", "kind = \"query\"");
    let (answers, log) = fixture.run(query("items")).await;
    assert_eq!(final_ids(&answers), ["good"]);
    for index in 0..5 {
        assert!(
            logged(&log, &format!("fixture: dropping item {index}:")),
            "item {index}: {log:?}"
        );
    }
    assert!(logged(&log, "missing field `id`"), "{log:?}");
    assert!(logged(&log, "missing field `label`"), "{log:?}");
    assert!(logged(&log, "missing field `actions`"), "{log:?}");
    assert!(logged(&log, r#""empty-actions" has no actions"#), "{log:?}");
}

#[tokio::test]
async fn a_response_to_another_request_is_ignored() {
    let fixture = Fixture::new("garbage", "kind = \"query\"");
    let (answers, log) = fixture.run(query("other-id")).await;
    assert_eq!(final_ids(&answers), ["right"]);
    assert!(
        logged(&log, "ignoring a response to request 8, not 7"),
        "{log:?}"
    );
}

#[tokio::test]
async fn a_result_doesnt_answer_a_query() {
    let fixture = Fixture::new("garbage", "kind = \"query\"");
    let (answers, log) = fixture.run(query("result")).await;
    assert_eq!(the_failure(&answers), &Failure::Invalid);
    assert!(logged(&log, "doesn't answer a query request"), "{log:?}");
}

#[tokio::test]
async fn exiting_without_an_answer_is_an_error() {
    let fixture = Fixture::new("garbage", "kind = \"query\"");
    let (answers, _) = fixture.run(query("silent")).await;
    match the_failure(&answers) {
        Failure::Exited(Some(status)) => assert!(status.success()),
        other => panic!("expected an exit, got {other:?}"),
    }
}

#[tokio::test]
async fn partial_answers_arrive_in_order_and_each_resets_the_timeout() {
    // Six gaps of 50 ms take longer than the timeout in total, but no one
    // gap does.
    let fixture = Fixture::new("stream", "kind = \"query\"\ntimeout_ms = 250");
    let (answers, _) = fixture.run(query("6")).await;
    let shape: Vec<(usize, bool)> = answers
        .iter()
        .map(|answer| match answer {
            Answer::Items { items, more } => (items.len(), *more),
            other => panic!("expected items, got {other:?}"),
        })
        .collect();
    assert_eq!(
        shape,
        [
            (1, true),
            (2, true),
            (3, true),
            (4, true),
            (5, true),
            (6, true),
            (7, false)
        ]
    );
    assert!(answers.last().unwrap().is_final());
    assert!(!answers[0].is_final());
}

#[tokio::test]
async fn a_timeout_after_a_partial_answer_kills_the_plugin() {
    let fixture = Fixture::new("stream", "kind = \"query\"\ntimeout_ms = 300");
    let (answers, _) = fixture.run(query("stall")).await;
    match &answers[..] {
        [
            Answer::Items { items, more: true },
            Answer::Failed(Failure::NoResponse(_)),
        ] => assert_eq!(ids(items), ["1"]),
        other => panic!("expected a partial then a timeout, got {other:?}"),
    }
    fixture.assert_gone();
}

/// The fixture's environment, as (variable, value) pairs.
async fn environment(fixture: &Fixture) -> Vec<(String, String)> {
    let (answers, _) = fixture.run(Method::list()).await;
    match &answers[..] {
        [Answer::Items { items, more: false }] => items
            .iter()
            .map(|item| (item.id.clone(), item.label.clone()))
            .collect(),
        other => panic!("expected items, got {other:?}"),
    }
}

fn expected_environment(fixture: &Fixture) -> Vec<(String, String)> {
    let dir = fixture.dirs.bundled_plugins.join("fixture");
    let path = |path: PathBuf| path.to_string_lossy().into_owned();
    vec![
        (
            "QL_CONFIG_DIR".into(),
            path(fixture.dirs.config_for("fixture")),
        ),
        ("QL_PLUGIN_DIR".into(), path(dir.clone())),
        ("QL_PLUGIN_ID".into(), "fixture".into()),
        ("QL_PROTOCOL".into(), "1".into()),
        (
            "QL_STATE_DIR".into(),
            path(fixture.dirs.state_for("fixture")),
        ),
        ("cwd".into(), path(dir)),
    ]
}

#[tokio::test]
async fn the_plugin_gets_every_variable_and_runs_in_its_folder() {
    let fixture = Fixture::new("env", "");
    let (launch, _) = fixture.launch();
    assert_eq!(launch.argv(), [fixture.plugin.bin().to_string_lossy()]);
    assert_eq!(environment(&fixture).await, expected_environment(&fixture));
}

#[tokio::test]
async fn a_login_plugin_starts_through_the_login_shell() {
    let fixture = Fixture::new("env", "env = \"login\"");
    let (launch, _) = fixture.launch();
    assert_eq!(&launch.argv()[..2], ["bash", "-lc"]);
    assert_eq!(
        launch.argv().last().unwrap(),
        &fixture.plugin.bin().to_string_lossy()
    );
    assert_eq!(environment(&fixture).await, expected_environment(&fixture));
}

#[tokio::test]
async fn a_binary_that_cant_start_is_an_error() {
    // Not executable.
    let fixture = Fixture::with_bin(&|dir| fs::write(dir.join("bin"), "").unwrap(), "");
    let (answers, log) = fixture.run(Method::list()).await;
    assert!(
        matches!(the_failure(&answers), Failure::Start(_)),
        "{answers:?}"
    );
    assert!(logged(&log, "fixture: can't start"), "{log:?}");
}

#[tokio::test]
async fn dropping_a_request_kills_its_plugin() {
    let fixture = Fixture::new("slow", "timeout_ms = 60000");
    let (launch, _) = fixture.launch();
    let request = Request::new(7, Method::list());
    let pid_file = fixture.dirs.state_for("fixture").join("pid");
    let run = oneshot(&launch, &request, |_| panic!("no answer is expected"));
    let wait_for_pid = async {
        while fs::read_to_string(&pid_file).map_or(true, |pid| pid.is_empty()) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::select! {
        () = run => panic!("the request should still be running"),
        () = wait_for_pid => {}
    }
    fixture.assert_gone();
}
