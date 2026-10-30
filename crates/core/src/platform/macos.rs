//! The macOS platform: apps and files go through `open`, terminals through
//! Terminal.app (or the app `QL_TERMINAL_APP` names), and the core keeps the
//! same XDG-style directories as on Linux, so plugins, config and the README
//! paths are the same on both.
//!
//! Each method builds its argv or script with a pure function, so tests
//! check them on any OS without running anything.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Dirs, Platform, command, non_empty, reap, strings, xdg_dirs};

/// The terminal app used when `QL_TERMINAL_APP` isn't set.
const DEFAULT_TERMINAL_APP: &str = "Terminal";

/// The login shell used when `SHELL` isn't one that takes `-lc` with
/// `"$@"`: macOS's default shell.
const DEFAULT_LOGIN_SHELL: &str = "/bin/zsh";

/// The macOS platform.
#[derive(Debug)]
pub struct MacOs {
    dirs: Dirs,
    /// The app terminal scripts open in, as `open -a` takes it.
    terminal_app: String,
    /// The shell `login_command` runs.
    login_shell: String,
    /// Where terminal scripts are written. Each script removes itself.
    scripts: PathBuf,
}

impl MacOs {
    /// The platform with its directories and settings read from the process
    /// environment. Fails when `HOME` isn't an absolute path.
    pub fn from_env() -> io::Result<MacOs> {
        Ok(MacOs::with_dirs(xdg_dirs(|name| env::var_os(name))?))
    }

    /// The platform with directories already worked out, as
    /// `--bundled-plugins` needs. The rest is read from the environment.
    pub fn with_dirs(dirs: Dirs) -> MacOs {
        let var = |name: &str| env::var(name).ok().filter(|value| !value.is_empty());
        MacOs {
            // They hold the user's command lines, so they're kept with the
            // core's own state rather than in a shared temporary folder. A
            // name starting with `.` can't be a plugin's state folder.
            scripts: dirs.plugin_state.join(".terminal-scripts"),
            dirs,
            terminal_app: var("QL_TERMINAL_APP").unwrap_or_else(|| DEFAULT_TERMINAL_APP.into()),
            login_shell: login_shell(var("SHELL").as_deref()),
        }
    }

    /// `cwd`, or home when there's none.
    fn cwd<'a>(&'a self, cwd: Option<&'a Path>) -> &'a Path {
        cwd.unwrap_or(&self.dirs.home)
    }

    /// Writes `text` as a new executable script, readable by its owner
    /// only, in a private folder.
    fn write_script(&self, text: &str) -> io::Result<PathBuf> {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.scripts)?;
        let path = self.scripts.join(format!(
            "run-{}-{}.command",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        // A script left by an earlier core with the same pid is replaced,
        // so it can't pass on its own contents or mode.
        match fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        fs::File::options()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&path)?
            .write_all(text.as_bytes())?;
        Ok(path)
    }
}

impl Platform for MacOs {
    fn spawn(&self, argv: &[String], cwd: Option<&Path>) -> io::Result<()> {
        detach(non_empty(argv)?.to_vec(), self.cwd(cwd))
    }

    fn terminal(&self, argv: &[String], cwd: Option<&Path>) -> io::Result<()> {
        let argv = non_empty(argv)?;
        let cwd = self.cwd(cwd);
        let script = self.write_script(&terminal_script(argv, cwd))?;
        let opened = detach(terminal_argv(&self.terminal_app, &script), &self.dirs.home);
        if opened.is_err() {
            let _ = fs::remove_file(&script);
        }
        opened
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
        // pbcopy sets the clipboard once stdin closes and then exits, so
        // waiting for it is quick, and a failed write or a failed pbcopy is
        // reported.
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let written = stdin.write_all(text.as_bytes());
        drop(stdin);
        let status = child.wait()?;
        written?;
        if !status.success() {
            return Err(io::Error::other(format!("pbcopy failed ({status})")));
        }
        Ok(())
    }

    fn login_command(&self, argv: &[String]) -> Vec<String> {
        login_argv(&self.login_shell, argv)
    }

    fn dirs(&self) -> &Dirs {
        &self.dirs
    }
}

/// Runs argv in `cwd` in a process group of its own, without waiting for
/// it, so it outlives the core and the app that started it.
fn detach(argv: Vec<String>, cwd: &Path) -> io::Result<()> {
    let child = command(&argv, cwd)
        .stdin(Stdio::null())
        .process_group(0)
        .spawn()?;
    reap(child);
    Ok(())
}

/// The script a terminal runs: it removes itself, changes to `cwd` and
/// replaces itself with argv. Every argument is single-quoted, so the
/// shell parses none of them.
pub(crate) fn terminal_script(argv: &[String], cwd: &Path) -> String {
    let command: Vec<String> = argv.iter().map(|arg| quote(arg)).collect();
    format!(
        "#!/bin/sh\nrm -f -- \"$0\"\ncd -- {} || exit 1\nexec {}\n",
        quote(&cwd.to_string_lossy()),
        command.join(" ")
    )
}

/// `open -a <app> <script>`: a terminal app runs a `.command` file it's
/// asked to open, in a new window.
pub(crate) fn terminal_argv(app: &str, script: &Path) -> Vec<String> {
    strings(["open", "-a", app])
        .chain([script.display().to_string()])
        .collect()
}

/// `open -a <id>`, where the id is an app bundle's path (what the `apps`
/// plugin lists) or an app name.
pub(crate) fn launch_app_argv(id: &str) -> Vec<String> {
    strings(["open", "-a", id]).collect()
}

/// `open <target>`.
pub(crate) fn open_argv(target: &str) -> Vec<String> {
    strings(["open", target]).collect()
}

/// `pbcopy`, which reads the text from stdin.
pub(crate) fn copy_argv() -> Vec<String> {
    strings(["pbcopy"]).collect()
}

/// `<shell> -lc 'exec "$@"' <shell> <argv>`: a login shell that replaces
/// itself with argv, so the profile's variables and `PATH` (Homebrew's,
/// say) apply and no argument is parsed by the shell.
pub(crate) fn login_argv(shell: &str, argv: &[String]) -> Vec<String> {
    let name = Path::new(shell)
        .file_name()
        .map_or_else(|| shell.to_owned(), |name| name.to_string_lossy().into());
    [shell.to_owned(), "-lc".into(), r#"exec "$@""#.into(), name]
        .into_iter()
        .chain(argv.iter().cloned())
        .collect()
}

/// The user's shell when it's an absolute path to a POSIX-style shell,
/// whose `-lc 'exec "$@"'` works as [`login_argv`] needs; zsh otherwise
/// (fish, say, spells `"$@"` differently).
pub(crate) fn login_shell(shell: Option<&str>) -> String {
    shell
        .filter(|shell| Path::new(shell).is_absolute())
        .filter(|shell| {
            Path::new(shell)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| matches!(name, "zsh" | "bash" | "sh" | "ksh" | "dash"))
        })
        .unwrap_or(DEFAULT_LOGIN_SHELL)
        .to_owned()
}

/// `text` in single quotes for `sh`.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    fn mac(scripts: PathBuf) -> MacOs {
        MacOs {
            dirs: xdg_dirs(|name| (name == "HOME").then(|| "/Users/me".into())).unwrap(),
            terminal_app: "Terminal".into(),
            login_shell: "/bin/zsh".into(),
            scripts,
        }
    }

    #[test]
    fn the_terminal_script_quotes_every_argument() {
        let script = terminal_script(
            &argv(&[
                "bash",
                "-ic",
                r#"eval "$1""#,
                "bash",
                "echo 'hi'; rm -rf $HOME",
            ]),
            Path::new("/Users/me/My Projects/it's"),
        );
        assert_eq!(
            script,
            concat!(
                "#!/bin/sh\n",
                "rm -f -- \"$0\"\n",
                r"cd -- '/Users/me/My Projects/it'\''s' || exit 1",
                "\n",
                r#"exec 'bash' '-ic' 'eval "$1"' 'bash' 'echo '\''hi'\''; rm -rf $HOME'"#,
                "\n"
            )
        );
    }

    #[test]
    fn the_terminal_script_runs_argv_as_given() {
        let dir = env::temp_dir().join(format!("ql-macos-script-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out");
        let tricky = "it's \"$HOME\" `x` ; $(y)";
        let script = terminal_script(
            &argv(&[
                "sh",
                "-c",
                r#"printf '%s|%s' "$PWD" "$1" > "$2""#,
                "sh",
                tricky,
            ])
            .into_iter()
            .chain([out.display().to_string()])
            .collect::<Vec<_>>(),
            &dir,
        );
        let path = mac(dir.clone()).write_script(&script).unwrap();
        let status = std::process::Command::new(&path).status().unwrap();
        let written = fs::read_to_string(&out).unwrap();
        let removed = !path.exists();
        fs::remove_dir_all(&dir).unwrap();
        assert!(status.success());
        assert_eq!(written, format!("{}|{tricky}", dir.display()));
        assert!(removed, "the script removes itself");
    }

    #[test]
    fn scripts_are_private_and_never_reused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = env::temp_dir().join(format!("ql-macos-private-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mac = mac(dir.clone());
        let first = mac.write_script("#!/bin/sh\n").unwrap();
        let second = mac.write_script("#!/bin/sh\n").unwrap();
        let dir_mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        let file_mode = fs::metadata(&first).unwrap().permissions().mode() & 0o777;
        fs::remove_dir_all(&dir).unwrap();
        assert_ne!(first, second);
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o700);
    }

    #[test]
    fn terminals_open_the_script_in_the_terminal_app() {
        assert_eq!(
            terminal_argv("iTerm", Path::new("/tmp/ql/run-1-0.command")),
            argv(&["open", "-a", "iTerm", "/tmp/ql/run-1-0.command"])
        );
    }

    #[test]
    fn apps_open_and_copy_go_through_open_and_pbcopy() {
        assert_eq!(
            launch_app_argv("/Applications/Safari.app"),
            argv(&["open", "-a", "/Applications/Safari.app"])
        );
        assert_eq!(
            open_argv("https://example.com"),
            argv(&["open", "https://example.com"])
        );
        assert_eq!(copy_argv(), argv(&["pbcopy"]));
    }

    #[test]
    fn an_empty_argv_is_refused_before_anything_starts() {
        let dir = env::temp_dir().join(format!("ql-macos-empty-{}", std::process::id()));
        let mac = mac(dir.clone());
        for result in [mac.spawn(&[], None), mac.terminal(&[], None)] {
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
        }
        assert!(!dir.exists(), "no script is written");
    }

    #[test]
    fn login_wraps_argv_without_a_shell_parsing_it() {
        assert_eq!(
            login_argv("/bin/zsh", &argv(&["pi", "$HOME; rm -rf /"])),
            argv(&[
                "/bin/zsh",
                "-lc",
                r#"exec "$@""#,
                "zsh",
                "pi",
                "$HOME; rm -rf /"
            ])
        );
    }

    #[test]
    fn the_login_shell_is_the_users_when_it_takes_posix_arguments() {
        assert_eq!(
            login_shell(Some("/opt/homebrew/bin/bash")),
            "/opt/homebrew/bin/bash"
        );
        assert_eq!(login_shell(Some("/bin/zsh")), "/bin/zsh");
        for shell in [None, Some("/opt/homebrew/bin/fish"), Some("zsh"), Some("")] {
            assert_eq!(login_shell(shell), "/bin/zsh", "{shell:?}");
        }
    }
}
