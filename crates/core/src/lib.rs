//! The quick launcher core. Built up from M5 on; see docs/milestones.md.

mod action;
pub mod discovery;
pub mod platform;

pub use action::{PluginEnv, PluginRef, run_action};
pub use discovery::{Plugin, Registry};
pub use platform::{Dirs, Platform};
