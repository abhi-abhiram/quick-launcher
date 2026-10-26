//! Running an action's `run` through the platform (*Actions* and
//! *Environment* in `docs/protocol.md`).

use std::io;
use std::path::{Path, PathBuf};

use quick_launcher_wire::Run;

use crate::platform::Platform;

/// A manifest's `env`: how the plugin, and its `exec` and `terminal`
/// actions, are started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PluginEnv {
    /// Inherit the core's environment.
    #[default]
    Session,
    /// Go through the platform's login shell.
    Login,
}

/// What `run_action` needs to know about the plugin that offered the action.
#[derive(Debug, Clone, Copy)]
pub struct PluginRef<'a> {
    /// The plugin folder, which relative `exec` paths resolve against.
    pub dir: &'a Path,
    pub env: PluginEnv,
}

/// Runs every form but `plugin` through `platform`.
///
/// `cwd` defaults to home, and a relative one resolves against home. In
/// `exec`, a relative program path (one with a `/`, such as `./run` or
/// `bin/tool`) resolves against the plugin folder; a bare name is looked up
/// on `PATH`. For `login` plugins, `exec` and `terminal` go through
/// [`Platform::login_command`]; `app`, `open` and `copy` run no plugin
/// command and are left alone.
///
/// A `plugin` run is the plugin's to answer, so the caller sends `activate`
/// instead; passing one here is an `InvalidInput` error, as is an empty argv.
pub fn run_action(
    platform: &(impl Platform + ?Sized),
    run: &Run,
    plugin: PluginRef,
) -> io::Result<()> {
    let home = &platform.dirs().home;
    match run {
        Run::Exec { argv, cwd } => {
            let mut argv = non_empty(argv)?.to_vec();
            if has_relative_path(&argv[0]) {
                argv[0] = plugin.dir.join(&argv[0]).to_string_lossy().into_owned();
            }
            let argv = wrap(platform, plugin, argv);
            platform.spawn(&argv, Some(&cwd_or_home(cwd.as_deref(), home)))
        }
        Run::Terminal { argv, cwd } => {
            let argv = wrap(platform, plugin, non_empty(argv)?.to_vec());
            platform.terminal(&argv, Some(&cwd_or_home(cwd.as_deref(), home)))
        }
        Run::App(id) => platform.launch_app(id),
        Run::Open(target) => platform.open(target),
        Run::Copy(text) => platform.copy(text),
        Run::Plugin(action) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("plugin action {action:?} is sent to the plugin, not run by the platform"),
        )),
    }
}

fn non_empty(argv: &[String]) -> io::Result<&[String]> {
    if argv.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "argv is empty"));
    }
    Ok(argv)
}

fn has_relative_path(program: &str) -> bool {
    program.contains('/') && Path::new(program).is_relative()
}

fn wrap(platform: &(impl Platform + ?Sized), plugin: PluginRef, argv: Vec<String>) -> Vec<String> {
    match plugin.env {
        PluginEnv::Session => argv,
        PluginEnv::Login => platform.login_command(&argv),
    }
}

fn cwd_or_home(cwd: Option<&str>, home: &Path) -> PathBuf {
    // Joining an absolute path replaces home, so this covers both.
    home.join(cwd.unwrap_or(""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Call, Dirs, Recording};

    const HOME: &str = "/home/me";
    const PLUGIN: &str = "/plugins/tools";

    fn recording() -> Recording {
        let config = PathBuf::from(HOME).join(".config/quick-launcher");
        Recording::new(Dirs {
            bundled_plugins: "/plugins".into(),
            user_plugins: config.join("plugins"),
            config_file: config.join("config.toml"),
            plugin_config: config,
            plugin_state: PathBuf::from(HOME).join(".local/state/quick-launcher"),
            home: HOME.into(),
        })
    }

    /// The calls `run` makes for a plugin with `env`.
    fn calls(run: Run, env: PluginEnv) -> Vec<Call> {
        let platform = recording();
        let plugin = PluginRef {
            dir: Path::new(PLUGIN),
            env,
        };
        run_action(&platform, &run, plugin).unwrap();
        platform.calls()
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    fn exec(parts: &[&str], cwd: Option<&str>) -> Run {
        Run::Exec {
            argv: argv(parts),
            cwd: cwd.map(String::from),
        }
    }

    fn terminal(parts: &[&str], cwd: Option<&str>) -> Run {
        Run::Terminal {
            argv: argv(parts),
            cwd: cwd.map(String::from),
        }
    }

    fn spawned(parts: &[&str], cwd: &str) -> Call {
        Call::Spawn {
            argv: argv(parts),
            cwd: Some(cwd.into()),
        }
    }

    #[test]
    fn exec_spawns_in_its_cwd() {
        assert_eq!(
            calls(exec(&["git", "pull"], Some("/repo")), PluginEnv::Session),
            [spawned(&["git", "pull"], "/repo")]
        );
    }

    #[test]
    fn cwd_defaults_to_home() {
        assert_eq!(
            calls(exec(&["htop"], None), PluginEnv::Session),
            [spawned(&["htop"], HOME)]
        );
        assert_eq!(
            calls(terminal(&["htop"], None), PluginEnv::Session),
            [Call::Terminal {
                argv: argv(&["htop"]),
                cwd: Some(HOME.into())
            }]
        );
    }

    #[test]
    fn a_relative_cwd_resolves_against_home() {
        assert_eq!(
            calls(exec(&["make"], Some("Projects/x")), PluginEnv::Session),
            [spawned(&["make"], "/home/me/Projects/x")]
        );
    }

    #[test]
    fn relative_exec_paths_resolve_against_the_plugin_folder() {
        assert_eq!(
            calls(exec(&["./run.sh", "./arg"], None), PluginEnv::Session),
            [spawned(&["/plugins/tools/./run.sh", "./arg"], HOME)]
        );
        assert_eq!(
            calls(exec(&["bin/tool"], None), PluginEnv::Session),
            [spawned(&["/plugins/tools/bin/tool"], HOME)]
        );
    }

    #[test]
    fn bare_and_absolute_programs_are_left_alone() {
        assert_eq!(
            calls(exec(&["firefox"], None), PluginEnv::Session),
            [spawned(&["firefox"], HOME)]
        );
        assert_eq!(
            calls(exec(&["/usr/bin/firefox"], None), PluginEnv::Session),
            [spawned(&["/usr/bin/firefox"], HOME)]
        );
    }

    #[test]
    fn terminal_argv_paths_are_not_resolved() {
        assert_eq!(
            calls(terminal(&["./run.sh"], Some("/repo")), PluginEnv::Session),
            [Call::Terminal {
                argv: argv(&["./run.sh"]),
                cwd: Some("/repo".into())
            }]
        );
    }

    #[test]
    fn login_plugins_wrap_exec_and_terminal() {
        assert_eq!(
            calls(exec(&["bin/tool", "x"], None), PluginEnv::Login),
            [spawned(&["login", "/plugins/tools/bin/tool", "x"], HOME)]
        );
        assert_eq!(
            calls(terminal(&["pi"], Some("/repo")), PluginEnv::Login),
            [Call::Terminal {
                argv: argv(&["login", "pi"]),
                cwd: Some("/repo".into())
            }]
        );
    }

    #[test]
    fn app_open_and_copy_are_never_wrapped() {
        for env in [PluginEnv::Session, PluginEnv::Login] {
            assert_eq!(
                calls(Run::App("firefox.desktop".into()), env),
                [Call::LaunchApp("firefox.desktop".into())]
            );
            assert_eq!(
                calls(Run::Open("https://example.com".into()), env),
                [Call::Open("https://example.com".into())]
            );
            assert_eq!(
                calls(Run::Copy("text".into()), env),
                [Call::Copy("text".into())]
            );
        }
    }

    #[test]
    fn plugin_runs_and_empty_argv_are_refused_without_a_call() {
        let platform = recording();
        let plugin = PluginRef {
            dir: Path::new(PLUGIN),
            env: PluginEnv::Session,
        };
        for run in [
            Run::Plugin("pin".into()),
            exec(&[], None),
            terminal(&[], None),
        ] {
            let err = run_action(&platform, &run, plugin).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        }
        assert!(platform.calls().is_empty());
    }
}
