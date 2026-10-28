//! The Omarchy platform: Hyprland under uwsm, with the XDG base directories.
//!
//! Each method builds its argv with a pure function, so tests check the
//! commands without running them.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use std::{env, thread};

use super::{Dirs, Platform, non_empty};

/// The platform the core runs on today.
#[derive(Debug, Clone)]
pub struct Omarchy {
    dirs: Dirs,
}

impl Omarchy {
    /// The platform with its directories read from the process environment.
    /// Fails when `HOME` isn't an absolute path.
    pub fn from_env() -> io::Result<Omarchy> {
        Ok(Omarchy {
            dirs: xdg_dirs(|name| env::var_os(name))?,
        })
    }

    /// The platform with directories already worked out, as
    /// `--bundled-plugins` needs.
    pub fn with_dirs(dirs: Dirs) -> Omarchy {
        Omarchy { dirs }
    }

    /// `cwd`, or home when there's none.
    fn cwd<'a>(&'a self, cwd: Option<&'a Path>) -> &'a Path {
        cwd.unwrap_or(&self.dirs.home)
    }
}

impl Platform for Omarchy {
    fn spawn(&self, argv: &[String], cwd: Option<&Path>) -> io::Result<()> {
        detach(spawn_argv(non_empty(argv)?), self.cwd(cwd))
    }

    fn terminal(&self, argv: &[String], cwd: Option<&Path>) -> io::Result<()> {
        let argv = non_empty(argv)?;
        let cwd = match cwd {
            Some(cwd) => cwd.to_path_buf(),
            None => {
                let printed = printed_by(TERMINAL_CWD_COMMAND, TERMINAL_CWD_TIMEOUT);
                terminal_cwd(printed.as_deref(), &self.dirs.home)
            }
        };
        detach(terminal_argv(argv, &cwd), &cwd)
    }

    fn launch_app(&self, id: &str) -> io::Result<()> {
        detach(launch_app_argv(id), &self.dirs.home)
    }

    fn open(&self, target: &str) -> io::Result<()> {
        detach(open_argv(target), &self.dirs.home)
    }

    fn copy(&self, text: &str) -> io::Result<()> {
        let mut child = command(&copy_argv(), &self.dirs.home)
            .stdin(Stdio::piped())
            .spawn()?;
        // wl-copy reads all of stdin before it forks to serve the clipboard,
        // so the text is written before returning: it isn't lost if the core
        // exits right after, and a failed write is reported.
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let written = stdin.write_all(text.as_bytes());
        drop(stdin);
        reap(child);
        written
    }

    fn login_command(&self, argv: &[String]) -> Vec<String> {
        login_argv(argv)
    }

    fn dirs(&self) -> &Dirs {
        &self.dirs
    }
}

/// `setsid uwsm-app -- <argv>`.
pub(crate) fn spawn_argv(argv: &[String]) -> Vec<String> {
    strings(["setsid", "uwsm-app", "--"])
        .chain(argv.iter().cloned())
        .collect()
}

/// `setsid uwsm-app -- xdg-terminal-exec --dir=<cwd> <argv>`.
pub(crate) fn terminal_argv(argv: &[String], cwd: &Path) -> Vec<String> {
    strings(["setsid", "uwsm-app", "--", "xdg-terminal-exec"])
        .chain([format!("--dir={}", cwd.display())])
        .chain(argv.iter().cloned())
        .collect()
}

/// Prints the folder of the focused terminal's shell, as Omarchy's own
/// new-terminal binding uses.
const TERMINAL_CWD_COMMAND: &str = "omarchy-cmd-terminal-cwd";

/// How long [`TERMINAL_CWD_COMMAND`] gets before the terminal opens in home
/// instead. Same as v0.
const TERMINAL_CWD_TIMEOUT: Duration = Duration::from_secs(2);

/// What `program` prints, or `None` when it can't run, fails, takes longer
/// than `timeout` or prints something that isn't UTF-8.
///
/// This blocks the caller, which for a `terminal` action is the core's
/// session loop: the launcher stays on screen until it returns. Keep the
/// timeout short, and don't call it anywhere that blocks more than that.
fn printed_by(program: &str, timeout: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                reap(child);
                return None;
            }
        }
    }
    let mut output = String::new();
    io::Read::read_to_string(&mut child.stdout.take()?, &mut output).ok()?;
    Some(output)
}

/// The folder a terminal opens in when its action names none: what
/// [`TERMINAL_CWD_COMMAND`] printed, without trailing whitespace, when
/// that's an existing absolute folder, home otherwise. This is v0's rule.
pub(crate) fn terminal_cwd(printed: Option<&str>, home: &Path) -> PathBuf {
    printed
        .map(|printed| Path::new(printed.trim_end()))
        .filter(|path| path.is_absolute() && path.is_dir())
        .map_or_else(|| home.to_path_buf(), Path::to_path_buf)
}

/// `setsid uwsm-app -- <id>`, which uwsm-app reads as a desktop entry id.
pub(crate) fn launch_app_argv(id: &str) -> Vec<String> {
    spawn_argv(&[id.to_owned()])
}

/// `xdg-open <target>`.
pub(crate) fn open_argv(target: &str) -> Vec<String> {
    strings(["xdg-open", target]).collect()
}

/// `wl-copy`, which reads the text from stdin.
pub(crate) fn copy_argv() -> Vec<String> {
    strings(["wl-copy"]).collect()
}

/// `bash -lc 'exec "$@"' bash <argv>`: a login shell that replaces itself
/// with argv, so the profile's variables and `PATH` apply and no argument is
/// parsed by the shell.
pub(crate) fn login_argv(argv: &[String]) -> Vec<String> {
    strings(["bash", "-lc", r#"exec "$@""#, "bash"])
        .chain(argv.iter().cloned())
        .collect()
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

fn strings<const N: usize>(parts: [&str; N]) -> impl Iterator<Item = String> {
    parts.into_iter().map(String::from)
}

/// Runs argv in `cwd` without waiting for it, reaping it on another thread.
fn detach(argv: Vec<String>, cwd: &Path) -> io::Result<()> {
    let child = command(&argv, cwd).stdin(Stdio::null()).spawn()?;
    reap(child);
    Ok(())
}

/// argv as a command in `cwd`. stdout is the frontend protocol, so nothing
/// started may write to it; stderr goes to the core's log.
fn command(argv: &[String], cwd: &Path) -> Command {
    let (program, args) = argv.split_first().expect("every builder names a program");
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    command
}

fn reap(mut child: Child) {
    thread::spawn(move || {
        let _ = child.wait();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    #[test]
    fn spawn_goes_through_uwsm_app() {
        assert_eq!(
            spawn_argv(&argv(&["foot", "-e", "htop"])),
            argv(&["setsid", "uwsm-app", "--", "foot", "-e", "htop"])
        );
    }

    #[test]
    fn terminal_passes_the_cwd_to_xdg_terminal_exec() {
        assert_eq!(
            terminal_argv(&argv(&["pi"]), Path::new("/home/me/Projects/x y")),
            argv(&[
                "setsid",
                "uwsm-app",
                "--",
                "xdg-terminal-exec",
                "--dir=/home/me/Projects/x y",
                "pi"
            ])
        );
    }

    #[test]
    fn a_terminal_without_cwd_opens_in_the_focused_terminals_folder() {
        let home = Path::new("/home/me");
        let focused = env::temp_dir();
        let printed = format!("{}\n", focused.display());
        assert_eq!(terminal_cwd(Some(&printed), home), focused);
    }

    #[test]
    fn a_terminal_falls_back_to_home_when_there_is_no_usable_folder() {
        let home = Path::new("/home/me");
        let missing = env::temp_dir().join("quick-launcher-no-such-folder");
        for printed in [
            None,
            Some(""),
            Some("\n"),
            Some("relative/folder\n"),
            Some(missing.to_str().unwrap()),
        ] {
            assert_eq!(terminal_cwd(printed, home), home, "{printed:?}");
        }
    }

    #[test]
    fn trailing_whitespace_is_dropped_as_in_v0() {
        let focused = env::temp_dir();
        let printed = format!("{} \r\n", focused.display());
        assert_eq!(terminal_cwd(Some(&printed), Path::new("/home/me")), focused);
    }

    /// A script in a fresh temporary folder that runs `body` with `sh`.
    fn script(name: &str, body: &str) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = env::temp_dir().join(format!(
            "quick-launcher-omarchy-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, path)
    }

    #[test]
    fn printed_by_returns_what_the_program_prints() {
        let (dir, path) = script("prints", "echo /some/folder");
        let printed = printed_by(path.to_str().unwrap(), Duration::from_secs(5));
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(printed.as_deref(), Some("/some/folder\n"));
    }

    #[test]
    fn printed_by_is_none_for_a_failure_bad_output_or_no_program() {
        let (dir, fails) = script("fails", "echo /some/folder; exit 1");
        let (dir2, binary) = script("binary", r"printf '\377\n'");
        let failed = printed_by(fails.to_str().unwrap(), Duration::from_secs(5));
        let not_utf8 = printed_by(binary.to_str().unwrap(), Duration::from_secs(5));
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(dir2).unwrap();
        assert_eq!(failed, None);
        assert_eq!(not_utf8, None);
        assert_eq!(
            printed_by("quick-launcher-no-such-program", Duration::from_secs(5)),
            None
        );
    }

    #[test]
    fn printed_by_gives_up_after_the_timeout() {
        let (dir, path) = script("hangs", "sleep 10; echo /some/folder");
        let started = Instant::now();
        let printed = printed_by(path.to_str().unwrap(), Duration::from_millis(200));
        let waited = started.elapsed();
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(printed, None);
        assert!(waited < Duration::from_secs(5), "waited {waited:?}");
    }

    #[test]
    fn apps_launch_by_desktop_entry_id() {
        assert_eq!(
            launch_app_argv("firefox.desktop"),
            argv(&["setsid", "uwsm-app", "--", "firefox.desktop"])
        );
    }

    #[test]
    fn open_and_copy() {
        assert_eq!(
            open_argv("https://example.com"),
            argv(&["xdg-open", "https://example.com"])
        );
        assert_eq!(copy_argv(), argv(&["wl-copy"]));
    }

    #[test]
    fn an_empty_argv_is_refused_before_anything_starts() {
        let omarchy = Omarchy::with_dirs(xdg_dirs(env(&[("HOME", "/home/me")])).unwrap());
        for result in [omarchy.spawn(&[], None), omarchy.terminal(&[], None)] {
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn login_wraps_argv_without_a_shell_parsing_it() {
        assert_eq!(
            login_argv(&argv(&["pi", "$HOME; rm -rf /"])),
            argv(&[
                "bash",
                "-lc",
                r#"exec "$@""#,
                "bash",
                "pi",
                "$HOME; rm -rf /"
            ])
        );
    }

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn dirs_default_under_home() {
        let dirs = xdg_dirs(env(&[("HOME", "/home/me")])).unwrap();
        assert_eq!(
            dirs,
            Dirs {
                bundled_plugins: "/home/me/.local/share/quick-launcher/plugins".into(),
                user_plugins: "/home/me/.config/quick-launcher/plugins".into(),
                config_file: "/home/me/.config/quick-launcher/config.toml".into(),
                plugin_config: "/home/me/.config/quick-launcher".into(),
                plugin_state: "/home/me/.local/state/quick-launcher".into(),
                home: "/home/me".into(),
            }
        );
        assert_eq!(
            dirs.config_for("pi"),
            Path::new("/home/me/.config/quick-launcher/pi")
        );
        assert_eq!(
            dirs.state_for("pi"),
            Path::new("/home/me/.local/state/quick-launcher/pi")
        );
    }

    #[test]
    fn dirs_follow_the_xdg_variables() {
        let dirs = xdg_dirs(env(&[
            ("HOME", "/home/me"),
            ("XDG_DATA_HOME", "/data"),
            ("XDG_CONFIG_HOME", "/config"),
            ("XDG_STATE_HOME", "/state"),
        ]))
        .unwrap();
        assert_eq!(
            dirs.bundled_plugins,
            Path::new("/data/quick-launcher/plugins")
        );
        assert_eq!(
            dirs.user_plugins,
            Path::new("/config/quick-launcher/plugins")
        );
        assert_eq!(
            dirs.config_file,
            Path::new("/config/quick-launcher/config.toml")
        );
        assert_eq!(dirs.state_for("pi"), Path::new("/state/quick-launcher/pi"));
    }

    #[test]
    fn empty_and_relative_xdg_variables_take_the_default() {
        let dirs = xdg_dirs(env(&[
            ("HOME", "/home/me"),
            ("XDG_DATA_HOME", ""),
            ("XDG_CONFIG_HOME", "relative/config"),
        ]))
        .unwrap();
        assert_eq!(
            dirs.bundled_plugins,
            Path::new("/home/me/.local/share/quick-launcher/plugins")
        );
        assert_eq!(
            dirs.config_file,
            Path::new("/home/me/.config/quick-launcher/config.toml")
        );
    }

    #[test]
    fn a_missing_or_relative_home_is_an_error() {
        assert!(xdg_dirs(env(&[])).is_err());
        assert!(xdg_dirs(env(&[("HOME", "me")])).is_err());
    }
}
