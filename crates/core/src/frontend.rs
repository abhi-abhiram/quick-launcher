//! The frontend protocol's messages (*Frontend protocol* and *Frontend
//! items* in `docs/protocol.md`). Unlike the wire format, this protocol is
//! public: any program that writes and reads these lines is a frontend.
//!
//! Within a major, message types and fields the core doesn't know are
//! ignored, so a newer frontend keeps working with an older core.

use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use quick_launcher_wire::Icon;

/// The frontend protocol's major version, sent as `v` in every message.
pub const VERSION: u32 = 1;

/// `max_items` when `hello` doesn't set it.
pub const DEFAULT_MAX_ITEMS: usize = 50;

/// A message from the frontend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    /// The first message. `v` is the major the frontend speaks.
    Hello {
        v: u32,
        client: String,
        client_version: String,
        max_items: usize,
    },
    /// A session starts, optionally straight into a mode.
    Open { mode: Option<String> },
    /// The typed text changed.
    Input {
        seq: u64,
        mode: Option<String>,
        text: String,
    },
    /// Leave the active mode.
    LeaveMode,
    /// Enter the selected hint's mode, or the first hint's.
    Complete { item: Option<String> },
    /// Run an action of an item.
    Activate { item: String, action: String },
    /// The session ended without running anything.
    Close,
    /// A `hello` that couldn't be read. It still gets a `ready`, and then
    /// the core exits, as on a version mismatch.
    InvalidHello { reason: String },
}

/// Why a line from the frontend was not a message the core acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unread {
    /// A message type this core doesn't know, which it ignores.
    UnknownType(String),
    /// Not JSON, or a known type with a field of the wrong type.
    Invalid(String),
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct HelloFields {
    v: u32,
    #[serde(default)]
    client: String,
    #[serde(default)]
    client_version: String,
    #[serde(default)]
    max_items: Option<usize>,
}

#[derive(Deserialize)]
struct OpenFields {
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Deserialize)]
struct InputFields {
    seq: u64,
    #[serde(default)]
    mode: Option<String>,
    text: String,
}

#[derive(Deserialize)]
struct CompleteFields {
    #[serde(default)]
    item: Option<String>,
}

#[derive(Deserialize)]
struct ActivateFields {
    item: String,
    action: String,
}

impl Incoming {
    /// Reads one line from the frontend.
    pub fn parse(line: &[u8]) -> Result<Incoming, Unread> {
        let value: Value =
            serde_json::from_slice(line).map_err(|error| Unread::Invalid(error.to_string()))?;
        let Envelope { kind } =
            Envelope::deserialize(&value).map_err(|error| Unread::Invalid(error.to_string()))?;
        let invalid = |error: serde_json::Error| Unread::Invalid(format!("{kind}: {error}"));
        Ok(match kind.as_str() {
            "hello" => match HelloFields::deserialize(&value) {
                Ok(f) => Incoming::Hello {
                    v: f.v,
                    client: f.client,
                    client_version: f.client_version,
                    max_items: f.max_items.unwrap_or(DEFAULT_MAX_ITEMS),
                },
                Err(error) => Incoming::InvalidHello {
                    reason: error.to_string(),
                },
            },
            "open" => Incoming::Open {
                mode: OpenFields::deserialize(&value).map_err(invalid)?.mode,
            },
            "input" => {
                let f = InputFields::deserialize(&value).map_err(invalid)?;
                Incoming::Input {
                    seq: f.seq,
                    mode: f.mode,
                    text: f.text,
                }
            }
            "leave_mode" => Incoming::LeaveMode,
            "complete" => Incoming::Complete {
                item: CompleteFields::deserialize(&value).map_err(invalid)?.item,
            },
            "activate" => {
                let f = ActivateFields::deserialize(&value).map_err(invalid)?;
                Incoming::Activate {
                    item: f.item,
                    action: f.action,
                }
            }
            "close" => Incoming::Close,
            _ => return Err(Unread::UnknownType(kind)),
        })
    }
}

/// A message to the frontend.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Outgoing {
    /// The reply to every `hello`.
    Ready { core_version: String },
    /// The complete state to draw.
    State(State),
    /// An action ran and the session is over.
    Done,
}

impl Outgoing {
    /// The message as one line, without its `\n`, with `v` first.
    pub fn to_line(&self) -> String {
        #[derive(Serialize)]
        struct Line<'a> {
            v: u32,
            #[serde(flatten)]
            message: &'a Outgoing,
        }
        serde_json::to_string(&Line {
            v: VERSION,
            message: self,
        })
        .expect("frontend messages always encode")
    }
}

/// What the frontend draws.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct State {
    /// The newest `input` applied.
    pub seq: u64,
    /// The active mode, or `None` for the plain search.
    pub mode: Option<Mode>,
    /// The typed text after the prefix, as typed.
    pub text: String,
    /// A plugin request for this mode and text is still outstanding.
    pub loading: bool,
    /// One user-readable line, drawn alongside the items.
    pub error: Option<String>,
    /// Best first, at most `max_items`.
    pub items: Vec<FrontendItem>,
}

/// The active mode, as the frontend shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Mode {
    pub id: String,
    pub name: String,
    /// Written as an object; left out, like `placeholder`, when the
    /// manifest doesn't set it, as for frontend items.
    #[serde(
        serialize_with = "explicit_icon",
        skip_serializing_if = "Option::is_none"
    )]
    pub icon: Option<Icon>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

/// What a frontend needs to draw a row. Actions carry no `run` or `after`:
/// how an action runs is the core's business.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FrontendItem {
    /// Identifies the item to the core; opaque to the frontend.
    #[serde(rename = "ref")]
    pub item_ref: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Always written as an object; left out when neither the item nor its
    /// plugin has one.
    #[serde(
        serialize_with = "explicit_icon",
        skip_serializing_if = "Option::is_none"
    )]
    pub icon: Option<Icon>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    pub disabled: bool,
    pub actions: Vec<FrontendAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FrontendAction {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// An icon as `{"glyph": …}`, `{"name": …}` or `{"path": …}`, never the
/// string shorthand plugins may use.
fn explicit_icon<S: Serializer>(icon: &Option<Icon>, serializer: S) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    #[serde(rename_all = "lowercase")]
    enum Explicit<'a> {
        Glyph(&'a str),
        Name(&'a str),
        Path(&'a str),
    }
    icon.as_ref()
        .map(|icon| match icon {
            Icon::Glyph(glyph) => Explicit::Glyph(glyph),
            Icon::Name(name) => Explicit::Name(name),
            Icon::Path(path) => Explicit::Path(path),
        })
        .serialize(serializer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> Result<Incoming, Unread> {
        Incoming::parse(value.to_string().as_bytes())
    }

    #[test]
    fn every_message_type_parses() {
        assert_eq!(
            parse(json!({"v":1,"type":"hello","client":"dev","client_version":"0.1"})),
            Ok(Incoming::Hello {
                v: 1,
                client: "dev".into(),
                client_version: "0.1".into(),
                max_items: DEFAULT_MAX_ITEMS,
            })
        );
        assert_eq!(
            parse(json!({"v":1,"type":"hello","max_items":3})),
            Ok(Incoming::Hello {
                v: 1,
                client: String::new(),
                client_version: String::new(),
                max_items: 3,
            })
        );
        assert_eq!(
            parse(json!({"v":1,"type":"open"})),
            Ok(Incoming::Open { mode: None })
        );
        assert_eq!(
            parse(json!({"v":1,"type":"open","mode":"pi"})),
            Ok(Incoming::Open {
                mode: Some("pi".into())
            })
        );
        assert_eq!(
            parse(json!({"v":1,"type":"input","seq":3,"mode":null,"text":"fire"})),
            Ok(Incoming::Input {
                seq: 3,
                mode: None,
                text: "fire".into()
            })
        );
        assert_eq!(
            parse(json!({"v":1,"type":"leave_mode"})),
            Ok(Incoming::LeaveMode)
        );
        assert_eq!(
            parse(json!({"v":1,"type":"complete"})),
            Ok(Incoming::Complete { item: None })
        );
        assert_eq!(
            parse(json!({"v":1,"type":"activate","item":"apps/firefox","action":"0"})),
            Ok(Incoming::Activate {
                item: "apps/firefox".into(),
                action: "0".into()
            })
        );
        assert_eq!(parse(json!({"v":1,"type":"close"})), Ok(Incoming::Close));
    }

    #[test]
    fn unknown_types_and_fields_are_ignored() {
        assert_eq!(
            parse(json!({"v":1,"type":"resize","width":3})),
            Err(Unread::UnknownType("resize".into()))
        );
        assert_eq!(
            parse(json!({"v":1,"type":"close","why":"escape"})),
            Ok(Incoming::Close)
        );
    }

    #[test]
    fn broken_messages_are_invalid() {
        for line in [
            "not json".to_owned(),
            json!({"v":1}).to_string(),
            json!({"v":1,"type":"input","seq":"one","text":""}).to_string(),
        ] {
            assert!(
                matches!(Incoming::parse(line.as_bytes()), Err(Unread::Invalid(_))),
                "{line}"
            );
        }
    }

    #[test]
    fn a_hello_without_v_is_an_invalid_hello() {
        assert!(matches!(
            parse(json!({"type":"hello"})),
            Ok(Incoming::InvalidHello { .. })
        ));
        assert!(matches!(
            parse(json!({"v":"one","type":"hello"})),
            Ok(Incoming::InvalidHello { .. })
        ));
    }

    #[test]
    fn outgoing_lines_carry_v_and_type() {
        assert_eq!(
            Outgoing::Ready {
                core_version: "0.1.0".into()
            }
            .to_line(),
            r#"{"v":1,"type":"ready","core_version":"0.1.0"}"#
        );
        assert_eq!(Outgoing::Done.to_line(), r#"{"v":1,"type":"done"}"#);
    }

    #[test]
    fn a_state_has_every_field_and_explicit_icons() {
        let state = State {
            seq: 12,
            mode: None,
            text: "fire".into(),
            loading: false,
            error: None,
            items: vec![
                FrontendItem {
                    item_ref: "apps/firefox.desktop".into(),
                    label: "Firefox".into(),
                    detail: Some("Web Browser".into()),
                    icon: Some(Icon::Glyph("󰈹".into())),
                    badge: None,
                    disabled: false,
                    actions: vec![FrontendAction {
                        id: "launch".into(),
                        title: "Launch".into(),
                        key: None,
                    }],
                },
                FrontendItem {
                    item_ref: "apps/x".into(),
                    label: "X".into(),
                    detail: None,
                    icon: None,
                    badge: Some("new".into()),
                    disabled: true,
                    actions: vec![FrontendAction {
                        id: "0".into(),
                        title: "Run".into(),
                        key: Some("ctrl+r".into()),
                    }],
                },
            ],
        };
        let line: Value = serde_json::from_str(&Outgoing::State(state).to_line()).unwrap();
        assert_eq!(
            line,
            json!({
                "v": 1, "type": "state", "seq": 12, "mode": null, "text": "fire",
                "loading": false, "error": null,
                "items": [
                    {"ref": "apps/firefox.desktop", "label": "Firefox",
                     "detail": "Web Browser", "icon": {"glyph": "󰈹"},
                     "disabled": false,
                     "actions": [{"id": "launch", "title": "Launch"}]},
                    {"ref": "apps/x", "label": "X", "badge": "new", "disabled": true,
                     "actions": [{"id": "0", "title": "Run", "key": "ctrl+r"}]},
                ]
            })
        );
    }

    #[test]
    fn a_mode_icon_is_explicit_too_and_unset_fields_are_left_out() {
        let mode = |icon, placeholder| {
            serde_json::to_value(Mode {
                id: "pi".into(),
                name: "pi".into(),
                icon,
                placeholder,
            })
            .unwrap()
        };
        assert_eq!(
            mode(
                Some(Icon::Name("folder".into())),
                Some("Open pi in…".into())
            ),
            json!({"id": "pi", "name": "pi", "icon": {"name": "folder"}, "placeholder": "Open pi in…"})
        );
        assert_eq!(mode(None, None), json!({"id": "pi", "name": "pi"}));
    }
}
