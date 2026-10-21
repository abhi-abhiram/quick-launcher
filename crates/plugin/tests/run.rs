//! `run` driven over in-memory pipes, as the core drives a plugin: list,
//! query, activate, missing methods, panics, partial answers, cancel and EOF.

use std::io::{self, BufReader, PipeReader, PipeWriter, Write};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use quick_launcher_plugin::{Action, Activate, Item, Outcome, Plugin, Query, Run, run_on};
use quick_launcher_wire::framing;
use serde_json::{Value, json};

/// A plugin running on its own thread, with the core's ends of its pipes.
struct Running {
    input: Option<PipeWriter>,
    output: BufReader<PipeReader>,
    thread: JoinHandle<io::Result<()>>,
}

fn start(plugin: impl Plugin + Send + 'static) -> Running {
    let (input_reader, input) = io::pipe().unwrap();
    let (output, output_writer) = io::pipe().unwrap();
    let thread = thread::spawn(move || run_on(plugin, input_reader, output_writer));
    Running {
        input: Some(input),
        output: BufReader::new(output),
        thread,
    }
}

impl Running {
    fn send(&mut self, request: Value) {
        let input = self.input.as_mut().expect("stdin still open");
        writeln!(input, "{request}").unwrap();
    }

    fn receive(&mut self) -> Value {
        let mut buf = Vec::new();
        framing::read_message(&mut self.output, &mut buf)
            .unwrap()
            .expect("a response before EOF")
    }

    fn ask(&mut self, request: Value) -> Value {
        self.send(request);
        self.receive()
    }

    /// Closes stdin, as the core does, and checks the plugin exits cleanly
    /// with nothing more written.
    fn finish(mut self) {
        drop(self.input.take());
        self.thread.join().unwrap().unwrap();
        let mut buf = Vec::new();
        let rest: Option<Value> = framing::read_message(&mut self.output, &mut buf).unwrap();
        assert_eq!(rest, None);
    }
}

fn list(id: u64) -> Value {
    json!({"v": 1, "id": id, "method": "list", "params": {}})
}

fn query(id: u64, text: &str) -> Value {
    json!({"v": 1, "id": id, "method": "query", "params": {"text": text}})
}

fn open(label: &str) -> Item {
    Item::new(label, label).action(Action::new("open", "Open").run(Run::open(label)))
}

/// Lists notes, ranks them with the SDK's matcher for a query, pins on
/// activate and panics when asked to.
struct Notes;

impl Plugin for Notes {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        Ok(vec![open("todo"), open("ideas")])
    }

    fn query(&mut self, query: &Query) -> anyhow::Result<Vec<Item>> {
        if query.text() == "panic" {
            panic!("asked to panic");
        }
        let items = [open("todo"), open("ideas"), open("today")];
        let matches = quick_launcher_plugin::matcher::filter(&items, query.text());
        Ok(matches.iter().map(|m| items[m.index].clone()).collect())
    }

    fn activate(&mut self, a: Activate) -> anyhow::Result<Outcome> {
        anyhow::ensure!(a.action == "pin", "unknown action {}", a.action);
        Ok(Outcome::query(format!("{:?} {}", a.item, a.text)))
    }
}

fn labels(response: &Value) -> Vec<&str> {
    response["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["label"].as_str().unwrap())
        .collect()
}

#[test]
fn list_answers_with_every_item() {
    let mut plugin = start(Notes);
    let response = plugin.ask(list(1));
    assert_eq!(response["v"], 1);
    assert_eq!(response["id"], 1);
    assert_eq!(labels(&response), ["todo", "ideas"]);
    assert_eq!(response.get("more"), None);
    plugin.finish();
}

#[test]
fn query_gets_the_text() {
    let mut plugin = start(Notes);
    assert_eq!(labels(&plugin.ask(query(1, "to"))), ["todo", "today"]);
    assert_eq!(
        labels(&plugin.ask(query(2, ""))),
        ["todo", "ideas", "today"]
    );
    plugin.finish();
}

#[test]
fn activate_gets_the_item_action_and_text() {
    let mut plugin = start(Notes);
    let response = plugin.ask(json!({
        "v": 1, "id": 4, "method": "activate",
        "params": {"item": "todo", "action": "pin", "text": "to"}
    }));
    assert_eq!(
        response,
        json!({"v": 1, "id": 4, "result": {"close": false, "query": "Some(\"todo\") to"}})
    );
    let response = plugin.ask(json!({
        "v": 1, "id": 5, "method": "activate",
        "params": {"item": null, "action": "pin", "text": "typed"}
    }));
    assert_eq!(response["result"]["query"], "None typed");
    plugin.finish();
}

#[test]
fn an_error_from_the_plugin_is_an_error_response() {
    let mut plugin = start(Notes);
    let response = plugin.ask(json!({
        "v": 1, "id": 3, "method": "activate",
        "params": {"item": "todo", "action": "delete", "text": ""}
    }));
    assert_eq!(
        response,
        json!({"v": 1, "id": 3, "error": {"message": "unknown action delete"}})
    );
    plugin.finish();
}

#[test]
fn methods_a_plugin_does_not_implement_answer_with_an_error() {
    struct Empty;
    impl Plugin for Empty {}

    let mut plugin = start(Empty);
    for request in [
        list(1),
        query(2, "x"),
        json!({"v": 1, "id": 3, "method": "activate", "params": {"item": "a", "action": "b"}}),
    ] {
        let id = request["id"].clone();
        let response = plugin.ask(request);
        assert_eq!(response["id"], id);
        assert!(response["error"]["message"].is_string(), "{response}");
    }
    plugin.finish();
}

#[test]
fn a_panic_costs_one_request() {
    let mut plugin = start(Notes);
    let response = plugin.ask(query(1, "panic"));
    assert_eq!(
        response,
        json!({"v": 1, "id": 1, "error": {"message": "the plugin crashed: asked to panic"}})
    );
    assert_eq!(labels(&plugin.ask(query(2, "idea"))), ["ideas"]);
    plugin.finish();
}

#[test]
fn partial_answers_come_before_the_final_one() {
    struct Slow;
    impl Plugin for Slow {
        fn query(&mut self, query: &Query) -> anyhow::Result<Vec<Item>> {
            query.partial(vec![open("first")])?;
            query.partial(vec![open("first"), open("second")])?;
            Ok(vec![open("first"), open("second"), open("third")])
        }
    }

    let mut plugin = start(Slow);
    plugin.send(query(9, "x"));
    let first = plugin.receive();
    assert_eq!(
        (first["id"].clone(), first["more"].clone()),
        (json!(9), json!(true))
    );
    assert_eq!(labels(&first), ["first"]);
    let second = plugin.receive();
    assert_eq!(second["more"], true);
    assert_eq!(labels(&second), ["first", "second"]);
    let last = plugin.receive();
    assert_eq!(last.get("more"), None);
    assert_eq!(labels(&last), ["first", "second", "third"]);
    plugin.finish();
}

#[test]
fn a_running_query_sees_its_cancel() {
    /// Says it started, then works until cancelled, for at most 10 seconds.
    struct Patient;
    impl Plugin for Patient {
        fn query(&mut self, query: &Query) -> anyhow::Result<Vec<Item>> {
            query.partial(vec![open("started")])?;
            let deadline = Instant::now() + Duration::from_secs(10);
            while !query.cancelled() {
                anyhow::ensure!(Instant::now() < deadline, "never saw the cancel");
                thread::sleep(Duration::from_millis(5));
            }
            Ok(vec![open("cancelled")])
        }
    }

    let mut plugin = start(Patient);
    plugin.send(query(1, "slow"));
    assert_eq!(labels(&plugin.receive()), ["started"]);
    // A cancel for a request that isn't running changes nothing.
    plugin.send(json!({"v": 1, "id": 2, "method": "cancel", "params": {"request": 99}}));
    plugin.send(json!({"v": 1, "id": 3, "method": "cancel", "params": {"request": 1}}));
    let last = plugin.receive();
    assert_eq!(last["id"], 1);
    assert_eq!(labels(&last), ["cancelled"]);
    // The next query starts uncancelled, and a cancel gets no response.
    plugin.send(query(4, "again"));
    assert_eq!(plugin.receive()["id"], 4);
    plugin.send(json!({"v": 1, "id": 5, "method": "cancel", "params": {"request": 4}}));
    assert_eq!(labels(&plugin.receive()), ["cancelled"]);
    plugin.finish();
}

#[test]
fn bad_lines_are_skipped_and_other_versions_refused() {
    let mut plugin = start(Notes);
    {
        let input = plugin.input.as_mut().unwrap();
        writeln!(input, "not json").unwrap();
        writeln!(input).unwrap();
    }
    let response = plugin.ask(json!({"v": 2, "id": 1, "method": "list", "params": {}}));
    assert_eq!(response["id"], 1);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("rebuild"),
        "{response}"
    );
    assert_eq!(labels(&plugin.ask(list(2))), ["todo", "ideas"]);
    plugin.finish();
}

#[test]
fn eof_with_no_requests_exits_cleanly() {
    start(Notes).finish();
}

#[test]
fn eof_after_a_request_still_answers_it() {
    // A oneshot plugin: the core writes one request and closes stdin.
    let mut plugin = start(Notes);
    plugin.send(list(1));
    drop(plugin.input.take());
    assert_eq!(labels(&plugin.receive()), ["todo", "ideas"]);
    plugin.finish();
}

#[test]
fn an_answer_too_large_to_send_becomes_an_error() {
    struct Huge;
    impl Plugin for Huge {
        fn list(&mut self) -> anyhow::Result<Vec<Item>> {
            Ok(vec![open(&"x".repeat(framing::MAX_LINE))])
        }
    }

    let mut plugin = start(Huge);
    let response = plugin.ask(list(1));
    assert_eq!(response["id"], 1);
    assert!(response["error"]["message"].is_string(), "{response}");
    plugin.finish();
}
