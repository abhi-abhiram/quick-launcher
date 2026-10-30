//! Everything OS-specific in the core (*Platform* and *Rules that keep the
//! core agnostic* in `docs/protocol.md`). No other module calls OS tools
//! directly.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;

// Both are built on every Unix, so each one's tests run in CI on Linux;
// [`Native`] picks the one the binary uses.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod omarchy;
#[cfg(any(test, feature = "recording-platform"))]
mod recording;

pub use macos::MacOs;
pub use omarchy::Omarchy;
#[cfg(any(test, feature = "recording-platform"))]
pub use recording::{Call, Recording};

/// The platform this build of the core runs on.
#[cfg(target_os = "macos")]
pub type Native = MacOs;
/// The platform this build of the core runs on.
#[cfg(not(target_os = "macos"))]
pub type Native = Omarchy;

/// The OS calls the core makes: [`Omarchy`] on Linux, [`MacOs`] on macOS.
///
/// `spawn` and `terminal` refuse an empty argv with `InvalidInput`.
pub trait Platform {
    /// Start argv detached from the core.
    fn spawn(&self, argv: &[String], cwd: Option<&Path>) -> io::Result<()>;
    /// Start argv in the user's terminal. With no `cwd`, the platform
    /// picks the folder: Omarchy uses the focused terminal's, or home;
    /// macOS uses home.
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

/// The core's directories from the XDG base directory variables, read
/// through `var`. Unset, empty and relative values take the XDG default, as
/// the spec says.
pub(crate) fn xdg_dirs(var: impl Fn(&str) -> Option<OsString>) -> io::Result<Dirs> {
    let home = var("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not an absolute path"))?;
    let base = |name: &str, default: &str| {
        var(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(default))
            .join("quick-launcher")
    };
    let data = base("XDG_DATA_HOME", ".local/share");
    let config = base("XDG_CONFIG_HOME", ".config");
    let state = base("XDG_STATE_HOME", ".local/state");
    Ok(Dirs {
        bundled_plugins: data.join("plugins"),
        user_plugins: config.join("plugins"),
        config_file: config.join("config.toml"),
        plugin_config: config,
        plugin_state: state,
        home,
    })
}

/// Runs argv in `cwd` without waiting for it, reaping it on another thread.
pub(super) fn detach(argv: Vec<String>, cwd: &Path) -> io::Result<()> {
    let child = command(&argv, cwd).stdin(Stdio::null()).spawn()?;
    reap(child);
    Ok(())
}

/// argv as a command in `cwd`. stdout is the frontend protocol, so nothing
/// started may write to it; stderr goes to the core's log.
pub(super) fn command(argv: &[String], cwd: &Path) -> Command {
    let (program, args) = argv.split_first().expect("every builder names a program");
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    command
}

pub(super) fn reap(mut child: Child) {
    thread::spawn(move || {
        let _ = child.wait();
    });
}

pub(super) fn strings<const N: usize>(parts: [&str; N]) -> impl Iterator<Item = String> {
    parts.into_iter().map(String::from)
}
