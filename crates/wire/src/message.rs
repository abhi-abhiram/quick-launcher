//! Requests from the core and responses from plugins (*Requests* and
//! *Responses* in `docs/protocol.md`).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::one_of::exactly_one;
use crate::{Item, PROTOCOL};

/// A request from the core: `{"v":1,"id":7,"method":"query","params":{…}}`.
///
/// The core always sends `params`, but a missing or `null` `params` reads
/// as `{}`, so a request piped in by hand can leave it out
/// (`{"v":1,"id":1,"method":"list"}`). Methods with required params still
/// need them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RequestFields")]
pub struct Request {
    /// The wire format major the sender speaks.
    pub v: u32,
    /// Unique per plugin process; echoed in every response to it.
    pub id: u64,
    #[serde(flatten)]
    pub method: Method,
}

impl Request {
    /// A request in the current protocol version.
    pub fn new(id: u64, method: Method) -> Self {
        Request {
            v: PROTOCOL,
            id,
            method,
        }
    }
}

/// A request as read, before its `params` are checked against its `method`.
#[derive(Deserialize)]
struct RequestFields {
    v: u32,
    id: u64,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

impl TryFrom<RequestFields> for Request {
    type Error = serde_json::Error;

    fn try_from(f: RequestFields) -> Result<Self, Self::Error> {
        let params = f
            .params
            .unwrap_or_else(|| Value::Object(Default::default()));
        let method = serde_json::from_value(json!({"method": f.method, "params": params}))?;
        Ok(Request {
            v: f.v,
            id: f.id,
            method,
        })
    }
}

/// A request's `method` and its `params`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "lowercase")]
pub enum Method {
    /// Every item of a `list` plugin. Its params are `{}`.
    List(ListParams),
    /// Items for the typed text, from a `query` plugin.
    Query(QueryParams),
    /// Run a `plugin` action.
    Activate(ActivateParams),
    /// Stop work on an earlier request. Has no response.
    Cancel(CancelParams),
}

impl Method {
    /// A `list` request's method, whose params are empty.
    pub fn list() -> Self {
        Method::List(ListParams {})
    }
}

/// `list` takes no params, but they are still sent as `{}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListParams {}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryParams {
    /// The query: what follows the prefix, trimmed.
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivateParams {
    /// The item's id, or `None` for the `[query_item]`.
    #[serde(default)]
    pub item: Option<String>,
    /// The action's id.
    pub action: String,
    /// The query, or for the `[query_item]` the text it was built from.
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelParams {
    /// The id of the request to stop.
    pub request: u64,
}

/// A plugin's answer to a request: `{"v":1,"id":7,"items":[…]}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ResponseFields")]
pub struct Response {
    pub v: u32,
    /// The id of the request this answers.
    pub id: u64,
    #[serde(flatten)]
    pub body: ResponseBody,
    /// A partial `query` answer: more responses to this request follow.
    #[serde(default, skip_serializing_if = "is_false")]
    pub more: bool,
}

impl Response {
    /// A final response in the current protocol version.
    pub fn new(id: u64, body: ResponseBody) -> Self {
        Response {
            v: PROTOCOL,
            id,
            body,
            more: false,
        }
    }
}

/// What a response carries: exactly one of `items`, `result` or `error`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResponseBody {
    /// The answer to `list` or `query`.
    Items(Vec<Item>),
    /// The answer to `activate`.
    Result(ActivateResult),
    /// The request failed.
    Error(ErrorBody),
}

/// Every key a response uses, so one with none or several of `items`,
/// `result` and `error` is rejected rather than read as whichever comes first.
#[derive(Deserialize)]
struct ResponseFields {
    v: u32,
    id: u64,
    #[serde(default)]
    items: Option<Vec<Item>>,
    #[serde(default)]
    result: Option<ActivateResult>,
    #[serde(default)]
    error: Option<ErrorBody>,
    #[serde(default)]
    more: bool,
}

impl TryFrom<ResponseFields> for Response {
    type Error = String;

    fn try_from(f: ResponseFields) -> Result<Self, Self::Error> {
        let mut bodies = Vec::new();
        bodies.extend(f.items.map(ResponseBody::Items));
        bodies.extend(f.result.map(ResponseBody::Result));
        bodies.extend(f.error.map(ResponseBody::Error));
        Ok(Response {
            v: f.v,
            id: f.id,
            body: exactly_one(bodies, "response", "items, result or error")?,
            more: f.more,
        })
    }
}

/// What the core does after a `plugin` action ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivateResult {
    /// Close the launcher.
    #[serde(default = "default_true")]
    pub close: bool,
    /// Re-request the mode's items. Implies the launcher stays open.
    #[serde(default, skip_serializing_if = "is_false")]
    pub refresh: bool,
    /// Replace the typed text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

impl Default for ActivateResult {
    fn default() -> Self {
        ActivateResult {
            close: true,
            refresh: false,
            query: None,
        }
    }
}

impl ActivateResult {
    /// Whether the launcher closes: `close` unless `refresh` keeps it open.
    pub fn closes(&self) -> bool {
        self.close && !self.refresh
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// One short, user-readable sentence.
    pub message: String,
}

fn default_true() -> bool {
    true
}

fn is_false(b: &bool) -> bool {
    !*b
}
