//! The quick launcher core. Built up from M5 on; see docs/milestones.md.

mod action;
pub mod app;
pub mod discovery;
pub mod frontend;
pub mod host;
pub mod log;
pub mod platform;
pub mod session;

pub use action::{PluginEnv, PluginRef, run_action};
pub use discovery::{Plugin, Registry};
pub use log::Log;
pub use platform::{Dirs, Platform};
pub use session::Session;
