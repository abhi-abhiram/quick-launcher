//! A plugin that breaks the wire format, written without the SDK, which
//! never would. It reads one request and answers according to its query
//! text (a `list` request counts as `garbage`):
//!
//! - `garbage`: a line that isn't JSON.
//! - `items`: items missing `id`, `label` or `actions`, one with no
//!   actions, one with two `run` forms, and one good item.
//! - `other-id`: a response to another request, then the real one.
//! - `result`: a `result`, which doesn't answer a query.
//! - `partial-result`: a `result` with `more: true`, which no answer to
//!   `activate` may have. Sent as an `activate` request's text.
//! - `silent`: exits without answering.

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

fn main() {
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line).unwrap();
    let request: Value = serde_json::from_str(&line).unwrap();
    let id = request["id"].as_u64().unwrap();
    let text = request["params"]["text"].as_str().unwrap_or("garbage");
    let copy = json!([{"title": "Copy", "run": {"copy": "x"}}]);
    let responses = match text {
        "garbage" => vec!["this is not json".to_owned()],
        "items" => vec![
            json!({"v": 1, "id": id, "items": [
                {"label": "No id", "actions": copy},
                {"id": "no-label", "actions": copy},
                {"id": "no-actions", "label": "No actions"},
                {"id": "empty-actions", "label": "Empty actions", "actions": []},
                {"id": "two-runs", "label": "Two runs",
                 "actions": [{"title": "Both", "run": {"copy": "x", "open": "y"}}]},
                {"id": "good", "label": "Good", "actions": copy},
            ]})
            .to_string(),
        ],
        "other-id" => vec![
            json!({"v": 1, "id": id + 1, "items": [{"id": "wrong", "label": "Wrong", "actions": copy}]})
                .to_string(),
            json!({"v": 1, "id": id, "items": [{"id": "right", "label": "Right", "actions": copy}]})
                .to_string(),
        ],
        "result" => vec![json!({"v": 1, "id": id, "result": {"close": true}}).to_string()],
        "partial-result" => {
            vec![json!({"v": 1, "id": id, "result": {"close": true}, "more": true}).to_string()]
        }
        "silent" => vec![],
        other => panic!("unknown text {other:?}"),
    };
    let mut out = io::stdout().lock();
    for response in responses {
        writeln!(out, "{response}").unwrap();
    }
}
