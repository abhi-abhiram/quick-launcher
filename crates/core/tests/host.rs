//! The plugin host, `oneshot` and `persistent`, against the fixture plugins
//! in `crates/core/fixtures`, which this test builds first.
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
use quick_launcher_core::host::{Answer, Failure, Launch, MAX_RESTARTS, Persistent, oneshot};
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
        fs::create_dir_all(&root).unwrap();
        // The plugin sees its folder with symlinks resolved (on macOS the
        // temp dir is under /var, a link to /private/var), so the
        // expected paths have them resolved too.
        let root = root.canonicalize().unwrap();
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
    /// stay a zombie briefly; that's not running either. Asks `ps`, which
    /// answers the same way on Linux and macOS.
    fn assert_gone(&self) {
        let pid = self.pid().to_string();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let ps = Command::new("ps")
                .args(["-o", "stat=", "-p", &pid])
                .output()
                .expect("ps runs");
            let stat = String::from_utf8_lossy(&ps.stdout);
            if !ps.status.success() || stat.trim_start().starts_with('Z') {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the plugin is still running ({})",
                stat.trim()
            );
            std::thread::sleep(Duration::from_millis(20));
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
    let fixture = Fixture::new("slow", "timeout_ms = 1000");
    let started = Instant::now();
    let (answers, log) = fixture.run(Method::list()).await;
    assert_eq!(
        the_failure(&answers),
        &Failure::NoResponse(Duration::from_millis(1000))
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(logged(&log, "didn't respond within 1000 ms"), "{log:?}");
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
    let fixture = Fixture::new("slow", "kind = \"query\"\ntimeout_ms = 1000");
    let (answers, log) = fixture.run(query("linger")).await;
    assert_eq!(final_ids(&answers), ["late"]);
    assert!(
        logged(&log, "still running 1000 ms after answering"),
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
async fn a_result_cant_be_partial() {
    let fixture = Fixture::new("garbage", "");
    let method = Method::Activate(ActivateParams {
        item: None,
        action: "run".into(),
        text: "partial-result".into(),
    });
    let (answers, log) = fixture.run(method).await;
    assert_eq!(the_failure(&answers), &Failure::Invalid);
    assert!(
        logged(&log, "only items can be a partial answer"),
        "{log:?}"
    );
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
    // Six gaps of 200 ms take longer than the timeout in total, but no one
    // gap does.
    let fixture = Fixture::new("stream", "kind = \"query\"\ntimeout_ms = 1000");
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
    // Long enough that a busy machine starting the fixture still gets the
    // partial answer out in time; only the hang after it should time out.
    let fixture = Fixture::new("stream", "kind = \"query\"\ntimeout_ms = 3000");
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

// Persistent plugins.

const PERSISTENT: &str = "kind = \"query\"\nlifetime = \"persistent\"";

/// A persistent plugin's task, and its answers as they arrive.
struct Running {
    plugin: Persistent,
    answers: tokio::sync::mpsc::UnboundedReceiver<(u64, Answer)>,
    log: Lines,
}

impl Running {
    fn start(fixture: &Fixture) -> Running {
        let (launch, log) = fixture.launch();
        let (sender, answers) = tokio::sync::mpsc::unbounded_channel();
        let plugin = Persistent::start(launch, move |id, answer| {
            let _ = sender.send((id, answer));
        });
        Running {
            plugin,
            answers,
            log,
        }
    }

    fn query(&self, id: u64, text: &str) {
        self.plugin.request(Request::new(id, query(text)));
    }

    /// The next answer, with the id of the request it answers.
    async fn next(&mut self) -> (u64, Answer) {
        tokio::time::timeout(Duration::from_secs(20), self.answers.recv())
            .await
            .expect("an answer within 20 s")
            .expect("the task is running")
    }

    /// The next answer, which must be a final one to request `id` with one
    /// item. Returns that item's id and label (the plugin's pid).
    async fn one_item(&mut self, id: u64) -> (String, String) {
        match self.next().await {
            (answered, Answer::Items { items, more: false }) if answered == id => {
                let [item] = &items[..] else {
                    panic!("expected one item, got {items:?}")
                };
                (item.id.clone(), item.label.clone())
            }
            other => panic!("expected items for request {id}, got {other:?}"),
        }
    }

    /// The next answer, which must be a failure of request `id`.
    async fn failure(&mut self, id: u64) -> Failure {
        match self.next().await {
            (answered, Answer::Failed(failure)) if answered == id => failure,
            other => panic!("expected request {id} to fail, got {other:?}"),
        }
    }

    /// Waits until the log has a line containing `text`.
    async fn wait_for_log(&self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !logged(&self.log.get(), text) {
            assert!(
                Instant::now() < deadline,
                "{text:?} not logged: {:?}",
                self.log.get()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Asserts nothing more arrives for a while.
    async fn assert_quiet(&mut self) {
        let more = tokio::time::timeout(Duration::from_millis(300), self.answers.recv()).await;
        assert!(more.is_err(), "unexpected answer {more:?}");
    }
}

impl Fixture {
    fn has_pid(&self) -> bool {
        self.dirs.state_for("fixture").join("pid").exists()
    }
}

#[tokio::test]
async fn a_persistent_plugin_starts_on_first_use_and_keeps_running() {
    let fixture = Fixture::new("persistent", PERSISTENT);
    let mut running = Running::start(&fixture);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!fixture.has_pid(), "started before its first request");

    running.query(40, "a");
    let (item, first) = running.one_item(40).await;
    assert_eq!(item, "a");
    // Ids are the caller's to choose; the process gets its own.
    running.query(7, "b");
    let (item, second) = running.one_item(7).await;
    assert_eq!(item, "b");
    assert_eq!(first, second, "both answered by one process");
    assert_eq!(fixture.pid().to_string(), first);

    running.plugin.shutdown().await.unwrap();
    fixture.assert_gone();
    let log = running.log.get();
    assert!(!logged(&log, "stopping it"), "it exited by itself: {log:?}");
}

#[tokio::test]
async fn a_newer_query_supersedes_the_older_one() {
    let fixture = Fixture::new("persistent", PERSISTENT);
    let mut running = Running::start(&fixture);
    running.query(1, "wait");
    running.plugin.cancel(1);
    running.query(2, "b");
    assert_eq!(running.one_item(2).await.0, "b");
    running
        .wait_for_log("fixture: cancelled while waiting")
        .await;
    running.assert_quiet().await;
    let log = running.log.get();
    assert!(!logged(&log, "ignoring a response"), "{log:?}");
    running.plugin.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_persistent_plugin_streams_partial_answers() {
    let fixture = Fixture::new("stream", PERSISTENT);
    let mut running = Running::start(&fixture);
    running.query(1, "3");
    let mut shape = Vec::new();
    loop {
        match running.next().await {
            (1, Answer::Items { items, more }) => {
                shape.push((items.len(), more));
                if !more {
                    break;
                }
            }
            other => panic!("expected items for request 1, got {other:?}"),
        }
    }
    assert_eq!(shape, [(1, true), (2, true), (3, true), (4, false)]);
    // The same process takes the next query.
    running.query(2, "0");
    match running.next().await {
        (2, Answer::Items { items, more: false }) => assert_eq!(ids(&items), ["1"]),
        other => panic!("expected the final answer to request 2, got {other:?}"),
    }
    running.plugin.shutdown().await.unwrap();
    fixture.assert_gone();
}

#[tokio::test]
async fn a_timeout_after_a_partial_answer_leaves_the_plugin_running() {
    let fixture = Fixture::new(
        "stream",
        "kind = \"query\"\nlifetime = \"persistent\"\ntimeout_ms = 3000",
    );
    let mut running = Running::start(&fixture);
    running.query(1, "stall");
    match running.next().await {
        (1, Answer::Items { items, more: true }) => assert_eq!(ids(&items), ["1"]),
        other => panic!("expected a partial answer, got {other:?}"),
    }
    assert_eq!(
        running.failure(1).await,
        Failure::NoResponse(Duration::from_millis(3000))
    );
    let pid = fixture.pid().to_string();
    let ps = Command::new("ps").args(["-p", &pid]).output().unwrap();
    assert!(ps.status.success(), "the plugin was stopped");
    // It never reads EOF while stalled, so shutting down kills it.
    running.plugin.shutdown().await.unwrap();
    fixture.assert_gone();
    let log = running.log.get();
    assert!(
        logged(&log, "after being told to stop; stopping it"),
        "{log:?}"
    );
}

#[tokio::test]
async fn a_timed_out_plugin_answers_the_next_request() {
    let fixture = Fixture::new(
        "persistent",
        "kind = \"query\"\nlifetime = \"persistent\"\ntimeout_ms = 1000",
    );
    let mut running = Running::start(&fixture);
    running.query(1, "a");
    let (_, pid) = running.one_item(1).await;
    running.query(2, "sleep 1200");
    assert_eq!(
        running.failure(2).await,
        Failure::NoResponse(Duration::from_millis(1000))
    );
    // Answered once the sleep is over, by the same process; the late
    // answer to request 2 is dropped.
    running.query(3, "b");
    assert_eq!(running.one_item(3).await, ("b".to_owned(), pid));
    running.assert_quiet().await;
    running.plugin.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_plugin_that_exits_is_restarted_on_the_next_request() {
    let fixture = Fixture::new("persistent", PERSISTENT);
    let mut running = Running::start(&fixture);
    running.query(1, "a");
    let (_, first) = running.one_item(1).await;
    running.query(2, "crash");
    match running.failure(2).await {
        Failure::Exited(Some(status)) => assert_eq!(status.code(), Some(3)),
        other => panic!("expected an exit, got {other:?}"),
    }
    running.query(3, "b");
    let (item, second) = running.one_item(3).await;
    assert_eq!(item, "b");
    assert_ne!(first, second, "a new process");
    assert!(logged(&running.log.get(), "fixture: restarting it"));
    running.plugin.shutdown().await.unwrap();
    fixture.assert_gone();
}

#[tokio::test]
async fn a_plugin_that_exits_too_often_stays_stopped() {
    let fixture = Fixture::new("persistent", PERSISTENT);
    let mut running = Running::start(&fixture);
    // The first start and three restarts.
    for id in 0..=MAX_RESTARTS as u64 {
        running.query(id, "crash");
        assert!(
            matches!(running.failure(id).await, Failure::Exited(_)),
            "crash {id}"
        );
    }
    for id in [10, 11] {
        running.query(id, "a");
        assert_eq!(running.failure(id).await, Failure::GaveUp);
    }
    let log = running.log.get();
    assert!(logged(&log, "won't be started again"), "{log:?}");
    assert_eq!(
        log.iter()
            .filter(|line| line.contains("restarting it"))
            .count(),
        MAX_RESTARTS
    );
    running.plugin.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutting_down_an_unused_plugin_starts_nothing() {
    let fixture = Fixture::new("persistent", PERSISTENT);
    let running = Running::start(&fixture);
    running.plugin.shutdown().await.unwrap();
    assert!(!fixture.has_pid());
    assert_eq!(running.log.get(), [] as [String; 0]);
}
