//! The `quick-launcher-core` program: the main loop that feeds frontend
//! messages and plugin answers to the [`Session`] and carries out what it
//! returns.
//!
//! The frontend writes to stdin and reads stdout; stderr is the log. EOF
//! on stdin ends the core with status 0.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use quick_launcher_wire::framing::FrameError;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::frontend::{Incoming, Unread};
use crate::host::lines::Lines;
use crate::host::{Answer, Launch, oneshot};
use crate::log::Log;
use crate::platform::{Omarchy, Platform};
use crate::run_action;
use crate::session::{Effect, Event, Session};
use crate::{Registry, discovery};

const USAGE: &str = "usage: quick-launcher-core [--bundled-plugins <dir>]";

/// How long plugin processes get to be stopped when the core exits.
const SHUTDOWN: Duration = Duration::from_millis(500);

/// Command-line options.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    /// Replaces the bundled plugin directory, for development.
    bundled_plugins: Option<PathBuf>,
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--bundled-plugins") => {
                let dir = args.next().ok_or("--bundled-plugins needs a directory")?;
                options.bundled_plugins = Some(PathBuf::from(dir));
            }
            Some("-h" | "--help") => return Err(USAGE.to_owned()),
            _ => return Err(format!("unknown argument {arg:?}\n{USAGE}")),
        }
    }
    Ok(options)
}

/// Runs the core with the process's arguments and environment.
pub fn main() -> ExitCode {
    let log = Log::stderr();
    let options = match parse_args(std::env::args_os().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let platform = match platform(&options) {
        Ok(platform) => platform,
        Err(error) => {
            log.write(format_args!("can't start: {error}"));
            return ExitCode::FAILURE;
        }
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the runtime starts");
    let code = runtime.block_on(run(platform, log));
    // Dropping the plugin tasks kills their processes. Reading stdin may
    // still hold a thread, which isn't waited for.
    runtime.shutdown_timeout(SHUTDOWN);
    ExitCode::from(code)
}

/// The platform: Omarchy, or with the `recording-platform` feature and
/// `QL_RECORD` set, a recording one that writes calls to that file.
fn platform(options: &Options) -> io::Result<Box<dyn Platform + Send + Sync>> {
    let mut dirs = Omarchy::from_env()?.dirs().clone();
    if let Some(dir) = &options.bundled_plugins {
        dirs.bundled_plugins = std::path::absolute(dir)?;
    }
    #[cfg(feature = "recording-platform")]
    if let Some(path) = std::env::var_os("QL_RECORD") {
        let recording = crate::platform::Recording::to_file(dirs, path.as_ref())?;
        return Ok(Box::new(recording));
    }
    Ok(Box::new(Omarchy::with_dirs(dirs)))
}

/// The main loop. Returns the exit status.
async fn run(platform: Box<dyn Platform + Send + Sync>, log: Log) -> u8 {
    let mut registry = Registry::new(platform.dirs().clone());
    log_warnings(&log, registry.warnings());
    let mut session = Session::new(registry.plugins().to_vec(), log.clone());
    let (answers, mut answered) = mpsc::unbounded_channel::<(u64, Answer)>();
    let mut requests: HashMap<u64, AbortHandle> = HashMap::new();
    let mut stdin = Lines::new(tokio::io::stdin());

    loop {
        let event = tokio::select! {
            line = stdin.next() => match line {
                Ok(Some(line)) if line.iter().all(u8::is_ascii_whitespace) => continue,
                Ok(Some(line)) => match Incoming::parse(&line) {
                    Ok(message) => {
                        if matches!(message, Incoming::Open { .. }) && registry.rescan_if_changed() {
                            log_warnings(&log, registry.warnings());
                            session.set_plugins(registry.plugins().to_vec());
                        }
                        Event::Frontend(message)
                    }
                    Err(Unread::UnknownType(kind)) => {
                        log.write(format_args!("ignoring a message of unknown type {kind:?}"));
                        continue;
                    }
                    Err(Unread::Invalid(reason)) => {
                        log.write(format_args!("ignoring an invalid message: {reason}"));
                        continue;
                    }
                },
                Ok(None) => return 0,
                Err(FrameError::TooLong) => {
                    log.write("ignoring a message longer than the line limit");
                    continue;
                }
                Err(error) => {
                    log.write(format_args!("can't read stdin: {error}"));
                    return 1;
                }
            },
            Some((request, answer)) = answered.recv() => {
                if answer.is_final() {
                    requests.remove(&request);
                }
                Event::Answer { request, answer }
            }
        };

        let mut pending = vec![event];
        while let Some(event) = pending.pop() {
            for effect in session.handle(event) {
                match effect {
                    Effect::Send(message) => {
                        if let Err(error) = send(&message.to_line()) {
                            log.write(format_args!("can't write to the frontend: {error}"));
                            return 1;
                        }
                    }
                    Effect::Request { plugin, request } => {
                        let launch = Launch::new(&*platform, &plugin, log.clone());
                        let answers = answers.clone();
                        let id = request.id;
                        let task = tokio::spawn(async move {
                            oneshot(&launch, &request, |answer| {
                                let _ = answers.send((id, answer));
                            })
                            .await;
                        });
                        requests.insert(id, task.abort_handle());
                    }
                    Effect::Cancel(request) => {
                        if let Some(task) = requests.remove(&request) {
                            task.abort();
                        }
                    }
                    Effect::Run { plugin, run } => {
                        let outcome = run_action(&*platform, &run, plugin.plugin_ref())
                            .map_err(|error| error.to_string());
                        pending.push(Event::Ran(outcome));
                    }
                    Effect::Exit(code) => return u8::try_from(code).unwrap_or(1),
                }
            }
        }
    }
}

/// Writes one line to the frontend and flushes it.
fn send(line: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(line.as_bytes())?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}

fn log_warnings(log: &Log, warnings: &[discovery::Warning]) {
    for warning in warnings {
        log.write(warning);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(args: &[&str]) -> Result<Options, String> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn no_arguments_use_the_platforms_directories() {
        assert_eq!(args(&[]), Ok(Options::default()));
    }

    #[test]
    fn bundled_plugins_takes_a_directory() {
        assert_eq!(
            args(&["--bundled-plugins", "target/plugins"]),
            Ok(Options {
                bundled_plugins: Some("target/plugins".into())
            })
        );
        assert!(args(&["--bundled-plugins"]).is_err());
    }

    #[test]
    fn unknown_arguments_are_refused() {
        assert!(args(&["--verbose"]).is_err());
    }
}
