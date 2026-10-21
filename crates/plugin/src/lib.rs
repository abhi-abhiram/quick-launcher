//! The quick launcher plugin SDK (*Plugin SDK* in `docs/protocol.md`).
//!
//! A plugin implements [`Plugin`] and hands it to [`run`], which deals with
//! the process side: reading requests, writing answers, catching panics.
//!
//! ```no_run
//! use quick_launcher_plugin::{Action, Item, Plugin, Run};
//!
//! struct Hello;
//!
//! impl Plugin for Hello {
//!     fn list(&mut self) -> anyhow::Result<Vec<Item>> {
//!         Ok(vec![Item::new("hello", "Say hello")
//!             .action(Action::new("copy", "Copy").run(Run::copy("hello")))])
//!     }
//! }
//!
//! fn main() { quick_launcher_plugin::run(Hello) }
//! ```
//!
//! Build it with `cargo build --release` and put the binary next to its
//! `plugin.toml`. To try it, pipe a request in:
//! `echo '{"v":1,"id":1,"method":"list","params":{}}' | ./hello`.

mod dirs;
mod item;
mod serve;

pub use dirs::{Dirs, dirs};
pub use item::{Action, After, Icon, Item, Outcome, Run};
pub use serve::{Activate, Plugin, Query, run, run_on};

/// The core's matcher, so a `query` plugin can rank its items the way the
/// core ranks a `list` plugin's.
pub use quick_launcher_wire::matcher;
