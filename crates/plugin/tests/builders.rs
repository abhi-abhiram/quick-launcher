//! What the builders put on the wire, read back from a `list` answer.

use std::io::{self, BufReader};
use std::thread;

use quick_launcher_plugin::{Action, After, Icon, Item, Outcome, Plugin, Run, run_on};
use quick_launcher_wire::{ActivateResult, framing};
use serde_json::{Value, json};

/// Lists `items`, and answers `activate` with `outcome`.
struct Fixed {
    items: Vec<Item>,
    outcome: Outcome,
}

impl Plugin for Fixed {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        Ok(self.items.clone())
    }

    fn activate(&mut self, _: quick_launcher_plugin::Activate) -> anyhow::Result<Outcome> {
        Ok(self.outcome.clone())
    }
}

/// Runs `plugin` over one request and returns its answer.
fn answer(plugin: Fixed, request: Value) -> Value {
    let input = format!("{request}\n").into_bytes();
    let (output, writer) = io::pipe().unwrap();
    let thread = thread::spawn(move || run_on(plugin, io::Cursor::new(input), writer));
    let mut output = BufReader::new(output);
    let mut buf = Vec::new();
    let response = framing::read_message(&mut output, &mut buf)
        .unwrap()
        .unwrap();
    thread.join().unwrap().unwrap();
    response
}

fn items(items: Vec<Item>) -> Value {
    let plugin = Fixed {
        items,
        outcome: Outcome::close(),
    };
    answer(
        plugin,
        json!({"v": 1, "id": 1, "method": "list", "params": {}}),
    )["items"]
        .clone()
}

fn result(outcome: Outcome) -> ActivateResult {
    let plugin = Fixed {
        items: Vec::new(),
        outcome,
    };
    let request =
        json!({"v": 1, "id": 1, "method": "activate", "params": {"item": null, "action": "a"}});
    serde_json::from_value(answer(plugin, request)["result"].clone()).unwrap()
}

#[test]
fn the_spec_item_is_built_exactly() {
    // The example under *Items* in docs/protocol.md.
    let dir = "/home/abhiram/Projects/quick-launcher";
    let item = Item::new(dir, "quick-launcher")
        .detail("~/Projects/quick-launcher")
        .icon("󰐃")
        .badge("pinned")
        .keywords(["launcher"])
        .disabled(false)
        .action(Action::new("open", "Open pi").run(Run::terminal(["pi"]).cwd(dir)))
        .action(Action::new("unpin", "Unpin").plugin().after(After::Refresh));
    assert_eq!(
        items(vec![item]),
        json!([{
            "id": "/home/abhiram/Projects/quick-launcher",
            "label": "quick-launcher",
            "detail": "~/Projects/quick-launcher",
            "icon": "󰐃",
            "badge": "pinned",
            "keywords": ["launcher"],
            "actions": [
                {"id": "open", "title": "Open pi",
                 "run": {"terminal": ["pi"], "cwd": "/home/abhiram/Projects/quick-launcher"}},
                {"id": "unpin", "title": "Unpin", "run": {"plugin": "unpin"}, "after": "refresh"}
            ]
        }])
    );
}

#[test]
fn every_optional_field_and_run_form() {
    let item = Item::new("a", "A")
        .icon(Icon::name("folder"))
        .keywords(["one"])
        .keywords(vec![String::from("two")])
        .disabled(true)
        .score(0.5)
        .action(
            Action::new("x", "Exec")
                .run(Run::exec(["./bin", "--flag"]).cwd("/tmp"))
                .key("ctrl+x"),
        )
        .action(Action::new("t", "Term").run(Run::terminal(["htop"])))
        .action(Action::new("ap", "App").run(Run::app("firefox.desktop")))
        .action(Action::new("o", "Open").run(Run::open("https://example.com")))
        .action(
            Action::new("c", "Copy")
                .run(Run::copy("text"))
                .after(After::Refresh),
        )
        .action(Action::new("p", "Pin"));
    let item = &items(vec![item])[0];
    assert_eq!(item["icon"], json!({"name": "folder"}));
    assert_eq!(item["keywords"], json!(["one", "two"]));
    assert_eq!(item["disabled"], true);
    assert_eq!(item["score"], 0.5);
    let runs: Vec<&Value> = item["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| &a["run"])
        .collect();
    assert_eq!(
        runs,
        [
            &json!({"exec": ["./bin", "--flag"], "cwd": "/tmp"}),
            &json!({"terminal": ["htop"]}),
            &json!({"app": "firefox.desktop"}),
            &json!({"open": "https://example.com"}),
            &json!({"copy": "text"}),
            // No `run` given: a plugin action with the action's own id.
            &json!({"plugin": "p"}),
        ]
    );
    assert_eq!(item["actions"][0]["key"], "ctrl+x");
    assert_eq!(item["actions"][4]["after"], "refresh");
}

#[test]
fn icons_take_every_form_and_a_string_is_a_glyph() {
    let icons = items(vec![
        open("a").icon("ab"),
        open("b").icon(String::from("folder")),
        open("c").icon(Icon::glyph("󰐃")),
        open("d").icon(Icon::path("/icons/d.svg")),
    ]);
    let icons: Vec<&Value> = icons
        .as_array()
        .unwrap()
        .iter()
        .map(|i| &i["icon"])
        .collect();
    assert_eq!(
        icons,
        [
            &json!("ab"),
            &json!("folder"),
            &json!("󰐃"),
            &json!({"path": "/icons/d.svg"})
        ]
    );
}

#[test]
fn cwd_only_applies_to_commands() {
    let item = open("a").action(Action::new("o", "Open").run(Run::open("/x").cwd("/tmp")));
    assert_eq!(
        items(vec![item])[0]["actions"][1]["run"],
        json!({"open": "/x"})
    );
}

#[test]
fn run_after_plugin_wins_and_plugin_after_run_wins() {
    let item = Item::new("a", "A")
        .action(Action::new("one", "One").plugin().run(Run::copy("x")))
        .action(Action::new("two", "Two").run(Run::copy("x")).plugin());
    let actions = &items(vec![item])[0]["actions"];
    assert_eq!(actions[0]["run"], json!({"copy": "x"}));
    assert_eq!(actions[1]["run"], json!({"plugin": "two"}));
}

#[test]
fn outcomes() {
    assert_eq!(
        result(Outcome::close()),
        ActivateResult {
            close: true,
            refresh: false,
            query: None
        }
    );
    let refresh = result(Outcome::refresh());
    assert!(refresh.refresh && !refresh.closes());
    let query = result(Outcome::query("pi: "));
    assert_eq!(query.query.as_deref(), Some("pi: "));
    assert!(!query.closes());
}

fn open(id: &str) -> Item {
    Item::new(id, id).action(Action::new("open", "Open").run(Run::open(id)))
}
