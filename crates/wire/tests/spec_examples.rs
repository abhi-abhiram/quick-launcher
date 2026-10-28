//! Every wire JSON example in docs/protocol.md, parsed into the wire types
//! and written back.

use quick_launcher_wire::{
    Action, ActivateParams, ActivateResult, After, CancelParams, ErrorBody, Icon, Item, Method,
    QueryParams, Request, Response, ResponseBody, Run,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::fmt::Debug;

/// Parses `json`, writes it back and checks that the result is `json` again
/// and parses to the same value.
fn round_trip<T>(json: &str) -> T
where
    T: DeserializeOwned + Serialize + PartialEq + Debug,
{
    let parsed: T = serde_json::from_str(json).expect("example parses");
    let written = serde_json::to_value(&parsed).unwrap();
    let original: Value = serde_json::from_str(json).unwrap();
    assert_eq!(written, original, "written back differently");
    let reparsed: T = serde_json::from_value(written).unwrap();
    assert_eq!(reparsed, parsed);
    parsed
}

// *Items*
const ITEM: &str = r#"{
  "id": "/home/abhiram/Projects/quick-launcher",
  "label": "quick-launcher",
  "detail": "~/Projects/quick-launcher",
  "icon": "󰐃",
  "badge": "pinned",
  "keywords": ["launcher"],
  "disabled": false,
  "actions": [
    {"id": "open", "title": "Open pi",
     "run": {"terminal": ["pi"], "cwd": "/home/abhiram/Projects/quick-launcher"}},
    {"id": "unpin", "title": "Unpin", "run": {"plugin": "unpin"}, "after": "refresh"}
  ]
}"#;

#[test]
fn item_example() {
    let parsed: Item = serde_json::from_str(ITEM).unwrap();
    assert_eq!(
        parsed,
        Item {
            id: "/home/abhiram/Projects/quick-launcher".into(),
            label: "quick-launcher".into(),
            detail: Some("~/Projects/quick-launcher".into()),
            icon: Some(Icon::Glyph("󰐃".into())),
            badge: Some("pinned".into()),
            keywords: vec!["launcher".into()],
            disabled: false,
            score: None,
            complete: None,
            actions: vec![
                Action {
                    id: Some("open".into()),
                    title: "Open pi".into(),
                    key: None,
                    run: Run::Terminal {
                        argv: vec!["pi".into()],
                        cwd: Some("/home/abhiram/Projects/quick-launcher".into()),
                    },
                    after: After::Close,
                },
                Action {
                    id: Some("unpin".into()),
                    title: "Unpin".into(),
                    key: None,
                    run: Run::Plugin("unpin".into()),
                    after: After::Refresh,
                },
            ],
        }
    );

    // `"disabled": false` is the default, so it isn't written back.
    let written = serde_json::to_value(&parsed).unwrap();
    let mut expected: Value = serde_json::from_str(ITEM).unwrap();
    expected.as_object_mut().unwrap().remove("disabled");
    assert_eq!(written, expected);
    assert_eq!(serde_json::from_value::<Item>(written).unwrap(), parsed);
}

// *Wire format*: the request used to debug a plugin by hand.
#[test]
fn list_request() {
    let request: Request = round_trip(r#"{"v":1,"id":1,"method":"list","params":{}}"#);
    assert_eq!(request, Request::new(1, Method::list()));
}

// *Requests*
#[test]
fn query_request() {
    let request: Request =
        round_trip(r#"{"v":1,"id":7,"method":"query","params":{"text":"2*21"}}"#);
    assert_eq!(
        request,
        Request::new(
            7,
            Method::Query(QueryParams {
                text: "2*21".into()
            })
        )
    );
}

// *Requests* table: `activate`, for an item and for the `[query_item]`.
#[test]
fn activate_requests() {
    let request: Request = round_trip(
        r#"{"v":1,"id":3,"method":"activate","params":{"item":"/p/x","action":"unpin","text":"x"}}"#,
    );
    assert_eq!(
        request.method,
        Method::Activate(ActivateParams {
            item: Some("/p/x".into()),
            action: "unpin".into(),
            text: "x".into(),
        })
    );

    let request: Request = round_trip(
        r#"{"v":1,"id":4,"method":"activate","params":{"item":null,"action":"run","text":"ls -la"}}"#,
    );
    assert_eq!(
        request.method,
        Method::Activate(ActivateParams {
            item: None,
            action: "run".into(),
            text: "ls -la".into(),
        })
    );
}

// *Requests* table: `cancel`.
#[test]
fn cancel_request() {
    let request: Request = round_trip(r#"{"v":1,"id":8,"method":"cancel","params":{"request":7}}"#);
    assert_eq!(request.method, Method::Cancel(CancelParams { request: 7 }));
}

// *Responses*
#[test]
fn responses() {
    let items: Response = round_trip(
        &format!(r#"{{"v":1,"id":7,"items":[{ITEM}]}}"#).replace(r#""disabled": false,"#, ""),
    );
    match items.body {
        ResponseBody::Items(items) => assert_eq!(items[0].label, "quick-launcher"),
        other => panic!("expected items, got {other:?}"),
    }
    assert!(!items.more);

    let result: Response = round_trip(r#"{"v":1,"id":8,"result":{"close":true}}"#);
    assert_eq!(
        result,
        Response::new(8, ResponseBody::Result(ActivateResult::default()))
    );

    let error: Response =
        round_trip(r#"{"v":1,"id":9,"error":{"message":"pinned.json is not valid JSON"}}"#);
    assert_eq!(
        error.body,
        ResponseBody::Error(ErrorBody {
            message: "pinned.json is not valid JSON".into()
        })
    );
}

// *Responses*: a partial `query` answer.
#[test]
fn partial_response() {
    let partial: Response = round_trip(r#"{"v":1,"id":7,"items":[],"more":true}"#);
    assert!(partial.more);
    assert_eq!(partial.body, ResponseBody::Items(vec![]));
}

// *Actions*: every `run` form in the table.
#[test]
fn run_forms() {
    let cases = [
        (
            json!({"exec": ["code", "/p"], "cwd": "/p"}),
            Run::Exec {
                argv: vec!["code".into(), "/p".into()],
                cwd: Some("/p".into()),
            },
        ),
        (
            json!({"exec": ["./bin/x"]}),
            Run::Exec {
                argv: vec!["./bin/x".into()],
                cwd: None,
            },
        ),
        (
            json!({"terminal": ["pi"], "cwd": "/p"}),
            Run::Terminal {
                argv: vec!["pi".into()],
                cwd: Some("/p".into()),
            },
        ),
        (
            json!({"app": "firefox.desktop"}),
            Run::App("firefox.desktop".into()),
        ),
        (
            json!({"open": "https://example.com"}),
            Run::Open("https://example.com".into()),
        ),
        (json!({"copy": "text"}), Run::Copy("text".into())),
        (json!({"plugin": "pin"}), Run::Plugin("pin".into())),
    ];
    for (value, run) in cases {
        assert_eq!(round_trip::<Run>(&value.to_string()), run, "{value}");
    }
}

// *Icons*
#[test]
fn icon_forms() {
    assert_eq!(round_trip::<Icon>(r#""󰐃""#), Icon::Glyph("󰐃".into()));
    assert_eq!(
        round_trip::<Icon>(r#"{"name":"folder"}"#),
        Icon::Name("folder".into())
    );
    assert_eq!(
        round_trip::<Icon>(r#"{"path":"/abs/icon.svg"}"#),
        Icon::Path("/abs/icon.svg".into())
    );
    // The long form of a string is written back as the short form.
    let long: Icon = serde_json::from_str(r#"{"glyph":"󰐃"}"#).unwrap();
    assert_eq!(long, Icon::Glyph("󰐃".into()));
    assert_eq!(serde_json::to_value(&long).unwrap(), json!("󰐃"));
}
