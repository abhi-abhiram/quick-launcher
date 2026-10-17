//! The wire rules M1 promises: unknown fields ignored, defaults filled in,
//! icons never guessed from length, one `run` form per action, and bounded
//! lines.

use std::io::{BufReader, Cursor, Read};

use quick_launcher_wire::framing::{self, FrameError, MAX_LINE};
use quick_launcher_wire::{
    Action, ActivateResult, After, Icon, Item, Method, PROTOCOL, Request, Response, ResponseBody,
    Run,
};
use serde_json::json;

#[test]
fn unknown_fields_are_ignored() {
    let request: Request = serde_json::from_value(json!({
        "v": 1, "id": 2, "method": "query", "trace": "abc",
        "params": {"text": "x", "cursor": 4}
    }))
    .unwrap();
    assert_eq!(request.id, 2);

    let response: Response = serde_json::from_value(json!({
        "v": 1, "id": 2, "elapsed_ms": 3,
        "items": [{
            "id": "a", "label": "A", "colour": "red",
            "icon": {"name": "folder", "size": 16},
            "actions": [{"title": "Go", "run": {"copy": "a", "mime": "text/plain"}, "hint": 1}]
        }]
    }))
    .unwrap();
    let ResponseBody::Items(items) = response.body else {
        panic!("expected items");
    };
    assert_eq!(items[0].icon, Some(Icon::Name("folder".into())));
    assert_eq!(items[0].actions[0].run, Run::Copy("a".into()));

    let result: ActivateResult =
        serde_json::from_value(json!({"close": false, "animate": true})).unwrap();
    assert!(!result.close);
}

#[test]
fn missing_optional_fields_take_defaults() {
    let item: Item = serde_json::from_value(json!({
        "id": "a", "label": "A",
        "actions": [{"title": "Go", "run": {"open": "/a"}}]
    }))
    .unwrap();
    assert_eq!(item.detail, None);
    assert_eq!(item.icon, None);
    assert_eq!(item.badge, None);
    assert!(item.keywords.is_empty());
    assert!(!item.disabled);
    assert_eq!(item.score, None);
    assert_eq!(
        item.actions[0],
        Action {
            id: None,
            title: "Go".into(),
            key: None,
            run: Run::Open("/a".into()),
            after: After::Close,
        }
    );

    let result: ActivateResult = serde_json::from_value(json!({})).unwrap();
    assert_eq!(
        result,
        ActivateResult {
            close: true,
            refresh: false,
            query: None
        }
    );
    assert!(result.closes());

    let refresh: ActivateResult = serde_json::from_value(json!({"refresh": true})).unwrap();
    assert!(refresh.close);
    assert!(!refresh.closes(), "refresh keeps the launcher open");

    let request: Request = serde_json::from_value(json!({
        "v": 1, "id": 5, "method": "activate", "params": {"action": "run"}
    }))
    .unwrap();
    let Method::Activate(params) = request.method else {
        panic!("expected activate");
    };
    assert_eq!(params.item, None);
    assert_eq!(params.text, "");
}

#[test]
fn required_fields_are_required() {
    for item in [
        json!({"label": "A", "actions": []}),
        json!({"id": "a", "actions": []}),
        json!({"id": "a", "label": "A"}),
    ] {
        assert!(
            serde_json::from_value::<Item>(item.clone()).is_err(),
            "{item}"
        );
    }
    assert!(serde_json::from_value::<Request>(json!({"v": 1, "id": 1, "params": {}})).is_err());
    assert!(
        serde_json::from_value::<Request>(json!({"v": 1, "id": 1, "method": "nope", "params": {}}))
            .is_err()
    );
    assert!(serde_json::from_value::<Response>(json!({"v": 1, "id": 1})).is_err());
    assert!(
        serde_json::from_value::<Response>(json!({
            "v": 1, "id": 1, "items": [], "error": {"message": "m"}
        }))
        .is_err(),
        "a response with two bodies is ambiguous"
    );
}

#[test]
fn icon_strings_are_glyphs_regardless_of_length() {
    for glyph in [
        "",
        "π",
        "󰐃",
        "👩‍💻",
        "folder",
        "a-long-string-that-looks-like-a-name",
    ] {
        let icon: Icon = serde_json::from_value(json!(glyph)).unwrap();
        assert_eq!(icon, Icon::Glyph(glyph.into()));
    }
}

#[test]
fn icon_objects_need_exactly_one_form() {
    for icon in [
        json!({}),
        json!({"size": 16}),
        json!({"name": "folder", "path": "/a.svg"}),
        json!({"glyph": "x", "name": "folder"}),
        json!(3),
    ] {
        assert!(
            serde_json::from_value::<Icon>(icon.clone()).is_err(),
            "{icon}"
        );
    }
}

#[test]
fn run_needs_exactly_one_form() {
    for run in [
        json!({}),
        json!({"cwd": "/p"}),
        json!({"exec": ["a"], "terminal": ["a"]}),
        json!({"app": "a.desktop", "open": "/p"}),
        json!({"copy": "x", "plugin": "pin"}),
    ] {
        let err = serde_json::from_value::<Run>(run.clone()).unwrap_err();
        assert!(err.to_string().contains("run"), "{run}: {err}");
    }
}

#[test]
fn constructors_use_the_current_protocol() {
    assert_eq!(Request::new(1, Method::list()).v, PROTOCOL);
    assert_eq!(Response::new(1, ResponseBody::Items(vec![])).v, PROTOCOL);
    assert_eq!(PROTOCOL, 1);
}

fn read_all_lines(input: &[u8]) -> Vec<Result<String, String>> {
    let mut reader = BufReader::with_capacity(64, Cursor::new(input.to_vec()));
    let mut buf = Vec::new();
    let mut lines = Vec::new();
    loop {
        match framing::read_line(&mut reader, &mut buf) {
            Ok(false) => return lines,
            Ok(true) => lines.push(Ok(String::from_utf8(buf.clone()).unwrap())),
            Err(e) => lines.push(Err(e.to_string())),
        }
    }
}

#[test]
fn read_line_splits_on_newlines() {
    assert_eq!(
        read_all_lines(b"one\ntwo\n\nlast"),
        vec![
            Ok("one".into()),
            Ok("two".into()),
            Ok("".into()),
            Ok("last".into())
        ]
    );
    assert!(read_all_lines(b"").is_empty());
}

#[test]
fn a_line_of_exactly_the_limit_is_accepted() {
    let mut input = vec![b'a'; MAX_LINE];
    input.push(b'\n');
    let mut reader = Cursor::new(input);
    let mut buf = Vec::new();
    assert!(framing::read_line(&mut reader, &mut buf).unwrap());
    assert_eq!(buf.len(), MAX_LINE);
}

/// A long run of bytes with no newline, produced without allocating it.
struct Endless {
    remaining: usize,
}

impl Read for Endless {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = out.len().min(self.remaining);
        out[..n].fill(b'x');
        self.remaining -= n;
        Ok(n)
    }
}

#[test]
fn a_line_over_the_limit_is_an_error_and_is_skipped() {
    let mut input = vec![b'a'; MAX_LINE + 1];
    input.extend_from_slice(b"\n{\"v\":1,\"id\":1,\"method\":\"list\",\"params\":{}}\n");
    let mut reader = BufReader::new(Cursor::new(input));
    let mut buf = Vec::new();

    let err = framing::read_message::<Request>(&mut reader, &mut buf).unwrap_err();
    assert!(matches!(err, FrameError::TooLong), "{err}");
    assert!(
        buf.capacity() <= MAX_LINE + 8 * 1024,
        "the long line wasn't kept"
    );

    let next: Request = framing::read_message(&mut reader, &mut buf)
        .unwrap()
        .unwrap();
    assert_eq!(next, Request::new(1, Method::list()));
}

#[test]
fn a_huge_line_is_not_held_in_memory() {
    // 64 MiB without a newline, then EOF.
    let mut reader = BufReader::new(Endless {
        remaining: 64 * MAX_LINE / 4,
    });
    let mut buf = Vec::new();
    let err = framing::read_line(&mut reader, &mut buf).unwrap_err();
    assert!(matches!(err, FrameError::TooLong));
    assert!(buf.capacity() <= MAX_LINE + 8 * 1024);
}

#[test]
fn read_message_skips_blank_lines_and_reports_bad_json() {
    let input = b"\n  \n{\"v\":1,\"id\":3,\"result\":{}}\nnot json\n";
    let mut reader = Cursor::new(&input[..]);
    let mut buf = Vec::new();
    let response: Response = framing::read_message(&mut reader, &mut buf)
        .unwrap()
        .unwrap();
    assert_eq!(response.id, 3);
    let err = framing::read_message::<Response>(&mut reader, &mut buf).unwrap_err();
    assert!(matches!(err, FrameError::Json(_)));
    assert!(
        framing::read_message::<Response>(&mut reader, &mut buf)
            .unwrap()
            .is_none()
    );
}

#[test]
fn write_message_writes_one_line() {
    let mut out = Vec::new();
    framing::write_message(&mut out, &Request::new(1, Method::list())).unwrap();
    framing::write_message(
        &mut out,
        &Response::new(1, ResponseBody::Result(ActivateResult::default())),
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "{\"v\":1,\"id\":1,\"method\":\"list\",\"params\":{}}\n\
         {\"v\":1,\"id\":1,\"result\":{\"close\":true}}\n"
    );

    // Newlines inside strings are escaped, so a message is always one line.
    let mut out = Vec::new();
    let copy = Run::Copy("a\nb".into());
    framing::write_message(&mut out, &copy).unwrap();
    assert_eq!(out.iter().filter(|&&b| b == b'\n').count(), 1);
}

#[test]
fn write_message_refuses_an_oversized_message() {
    let mut out = Vec::new();
    let copy = Run::Copy("x".repeat(MAX_LINE));
    let err = framing::write_message(&mut out, &copy).unwrap_err();
    assert!(matches!(err, FrameError::TooLong));
    assert!(out.is_empty(), "nothing was written");
}
