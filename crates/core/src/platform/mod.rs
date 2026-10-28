//! Everything OS-specific in the core (*Platform* and *Rules that keep the
//! core agnostic* in `docs/protocol.md`). No other module calls OS tools
//! directly.

use std::io;
use std::path::{Path, PathBuf};

mod omarchy;
#[cfg(any(test, feature = "recording-platform"))]
mod recording;

pub use omarchy::Omarchy;
#[cfg(any(test, feature = "recording-platform"))]
pub use recording::{Call, Recording};

/// The OS calls the core makes. `Omarchy` is the only implementation.
///
/// `spawn` and `terminal` refuse an empty argv with `InvalidInput`.
pub trait Platform {
    /// Start argv detached from the core.
    fn spawn(&self, argv: &[String], cwd: Option<&Path>) -> io::Result<()>;
    /// Start argv in the user's terminal. With no `cwd`, the platform
    /// picks the folder: Omarchy uses the focused terminal's, or home.
    fn terminal(&self, argv: &[String], cwd: Option<&Path>) -> io::Result<()>;
    /// Launch an installed app by its platform id.
    fn launch_app(&self, id: &str) -> io::Result<()>;
    /// Open a URL or path with its default handler.
    fn open(&self, target: &str) -> io::Result<()>;
    /// Put text on the clipboard.
    fn copy(&self, text: &str) -> io::Result<()>;
    /// Wrap argv so it runs with the user's login environment. Used for
    /// `login` plugins and for their `exec` and `terminal` actions.
    fn login_command(&self, argv: &[String]) -> Vec<String>;
    /// Where bundled plugins, user plugins and config, and state live.
    fn dirs(&self) -> &Dirs;
}

/// Where the core finds plugins and keeps their files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    /// Bundled plugins, one folder per id, read first.
    pub bundled_plugins: PathBuf,
    /// The user's plugins, one folder per id. One here replaces a bundled
    /// plugin with the same id.
    pub user_plugins: PathBuf,
    /// The user's `config.toml`.
    pub config_file: PathBuf,
    /// Holds each plugin's config folder, named by its id.
    pub plugin_config: PathBuf,
    /// Holds each plugin's state folder, named by its id.
    pub plugin_state: PathBuf,
    /// The user's home folder: where `exec` actions run without a `cwd`, and
    /// what a relative `cwd` resolves against.
    pub home: PathBuf,
}

impl Dirs {
    /// A plugin's `QL_CONFIG_DIR`.
    pub fn config_for(&self, id: &str) -> PathBuf {
        self.plugin_config.join(id)
    }

    /// A plugin's `QL_STATE_DIR`.
    pub fn state_for(&self, id: &str) -> PathBuf {
        self.plugin_state.join(id)
    }
}

/// argv, or an `InvalidInput` error when it's empty.
pub(crate) fn non_empty(argv: &[String]) -> io::Result<&[String]> {
    if argv.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "argv is empty"));
    }
    Ok(argv)
}
