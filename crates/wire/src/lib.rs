//! The wire format between the quick launcher core and plugins built with the
//! SDK. It is private to the two of them (see *Wire format* in
//! `docs/protocol.md`), so plugins never use this crate directly. It also holds
//! the [`matcher`], which the SDK re-exports.
//!
//! Every type ignores fields it doesn't know and fills missing optional fields
//! with their defaults, so a plugin built with an older SDK keeps working.

pub mod framing;
mod item;
pub mod matcher;
mod message;
mod one_of;

pub use item::{Action, After, Icon, Item, Run};
pub use message::{
    ActivateParams, ActivateResult, CancelParams, ErrorBody, ListParams, Method, QueryParams,
    Request, Response, ResponseBody,
};

/// The wire format's major version, sent as `v` in every message and as
/// `protocol` in a plugin's manifest.
pub const PROTOCOL: u32 = 1;
