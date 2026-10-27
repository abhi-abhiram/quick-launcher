//! Running plugin processes (*Lifetimes*, *Environment*, *Transport* and
//! *Errors and timeouts* in `docs/protocol.md`). M7 covers `oneshot`
//! plugins: one process per request.
//!
//! Nothing here knows about sessions or modes. A request goes in, and each
//! response comes back as an [`Answer`] as soon as it arrives.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use quick_launcher_wire::framing::FrameError;
use quick_launcher_wire::{ActivateResult, ErrorBody, Item, Method, PROTOCOL, Request};
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep_until, timeout};

use crate::action::PluginEnv;
use crate::discovery::Plugin;
use crate::log::Log;
use crate::platform::Platform;

mod lines;

use lines::Lines;

/// After a plugin exits, how long to wait for the rest of its stderr. A
/// process it started could hold stderr open for good.
const STDERR_GRACE: Duration = Duration::from_millis(200);

/// How to start one plugin's process: its argv, folder and environment.
#[derive(Debug, Clone)]
pub struct Launch {
    id: String,
    argv: Vec<String>,
    dir: PathBuf,
    env: Vec<(&'static str, OsString)>,
    timeout: Duration,
    log: Log,
}

impl Launch {
    /// How `plugin` starts on `platform`: its binary in its folder, through
    /// [`Platform::login_command`] for `env = "login"`, with the `QL_*`
    /// variables set and every other variable inherited from the core.
    pub fn new(platform: &(impl Platform + ?Sized), plugin: &Plugin, log: Log) -> Launch {
        let id = plugin.id().to_owned();
        let dirs = platform.dirs();
        let bin = vec![plugin.bin().to_string_lossy().into_owned()];
        let argv = match plugin.manifest.env {
            PluginEnv::Session => bin,
            PluginEnv::Login => platform.login_command(&bin),
        };
        let env = vec![
            ("QL_PROTOCOL", PROTOCOL.to_string().into()),
            ("QL_PLUGIN_ID", id.clone().into()),
            ("QL_PLUGIN_DIR", plugin.dir.clone().into()),
            ("QL_CONFIG_DIR", dirs.config_for(&id).into()),
            ("QL_STATE_DIR", dirs.state_for(&id).into()),
        ];
        Launch {
            id,
            argv,
            dir: plugin.dir.clone(),
            env,
            timeout: Duration::from_millis(plugin.manifest.timeout_ms),
            log,
        }
    }

    /// The plugin's id, which prefixes everything logged about it.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The argv the process is started with.
    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    fn start(&self) -> io::Result<Child> {
        let (program, args) = self
            .argv
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "argv is empty"))?;
        Command::new(program)
            .args(args)
            .current_dir(&self.dir)
            .envs(self.env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // A dropped request (a newer query, the core exiting) leaves no
            // process behind.
            .kill_on_drop(true)
            .spawn()
    }

    fn log(&self, line: impl fmt::Display) {
        self.log.write(format_args!("{}: {line}", self.id));
    }
}

/// One response to a request, as the core uses it.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// Items for `list` or `query`, already checked: any that broke the
    /// rules were dropped and logged. `more` marks a partial answer, which
    /// replaces the one before it; more answers follow.
    Items { items: Vec<Item>, more: bool },
    /// The outcome of `activate`.
    Result(ActivateResult),
    /// The request failed. Nothing follows.
    Failed(Failure),
}

impl Answer {
    /// Whether this ends the request.
    pub fn is_final(&self) -> bool {
        !matches!(self, Answer::Items { more: true, .. })
    }
}

/// Why a request got no usable answer. Each displays as one short sentence
/// the state's `error` can show after the plugin's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The process couldn't be started.
    Start(String),
    /// The plugin answered with an error.
    Plugin(String),
    /// A response line couldn't be read: not JSON, too long, or not a
    /// response to this kind of request. The details are logged.
    Invalid,
    /// No response within `timeout_ms` of the request or the last partial
    /// answer. The process was killed.
    NoResponse(Duration),
    /// The process ended without a final answer.
    Exited(Option<ExitStatus>),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Start(error) => write!(f, "couldn't be started: {error}"),
            Failure::Plugin(message) => f.write_str(message),
            Failure::Invalid => f.write_str("sent an answer the launcher can't read"),
            Failure::NoResponse(after) => {
                write!(f, "didn't respond within {} ms", after.as_millis())
            }
            Failure::Exited(Some(status)) => write!(f, "stopped without answering ({status})"),
            Failure::Exited(None) => f.write_str("stopped without answering"),
        }
    }
}

/// Runs a `oneshot` plugin for `request` and passes each response to
/// `answer` as it arrives, ending with one where [`Answer::is_final`].
/// Returns once the process is reaped.
///
/// The steps follow *Transport*: write the request and close stdin, read
/// responses until the final one, then wait up to `timeout_ms` for the
/// process to exit and kill it if it hasn't. `timeout_ms` also limits each
/// wait for a response; when it runs out the process is killed and the
/// request fails with [`Failure::NoResponse`]. Responses to other request
/// ids are logged and ignored, and stderr goes to the log line by line.
///
/// `request` must be one that has a response, so not `cancel`. Dropping the
/// future kills the process.
pub async fn oneshot(launch: &Launch, request: &Request, mut answer: impl FnMut(Answer)) {
    let mut child = match launch.start() {
        Ok(child) => child,
        Err(error) => {
            launch.log(format_args!("can't start {:?}: {error}", launch.argv));
            answer(Answer::Failed(Failure::Start(error.to_string())));
            return;
        }
    };
    let mut stdout = Lines::new(child.stdout.take().expect("stdout is piped"));
    let mut stderr = Some(Lines::new(child.stderr.take().expect("stderr is piped")));
    let mut deadline = Instant::now() + launch.timeout;

    let mut stdin = child.stdin.take().expect("stdin is piped");
    let mut line = serde_json::to_vec(request).expect("requests always encode");
    line.push(b'\n');
    // A plugin that exits without reading its request is noticed below, as
    // an exit without an answer, so a failed write needs no handling here.
    let written = timeout(launch.timeout, async {
        let _ = stdin.write_all(&line).await;
        drop(stdin);
    });
    if written.await.is_err() {
        timed_out(launch, child, &mut stderr, &mut answer).await;
        return;
    }

    loop {
        tokio::select! {
            line = stdout.next() => {
                let line = match line {
                    Ok(Some(line)) if line.iter().all(u8::is_ascii_whitespace) => continue,
                    Ok(Some(line)) => line,
                    Ok(None) => {
                        let status = reap(launch, child, &mut stderr).await;
                        answer(Answer::Failed(Failure::Exited(status)));
                        return;
                    }
                    Err(FrameError::Io(error)) => {
                        launch.log(format_args!("can't read its output: {error}"));
                        answer(Answer::Failed(Failure::Invalid));
                        break;
                    }
                    Err(error) => {
                        launch.log(format_args!("unreadable response: {error}"));
                        answer(Answer::Failed(Failure::Invalid));
                        break;
                    }
                };
                let Some(next) = read_response(launch, request, &line) else {
                    continue;
                };
                let done = next.is_final();
                answer(next);
                if done {
                    break;
                }
                deadline = Instant::now() + launch.timeout;
            }
            Some(line) = next_stderr(&mut stderr) => forward(launch, line),
            () = sleep_until(deadline) => {
                timed_out(launch, child, &mut stderr, &mut answer).await;
                return;
            }
        }
    }
    // Nothing more is read from stdout, so a plugin still writing gets a
    // broken pipe rather than blocking until it's killed.
    drop(stdout);
    reap(launch, child, &mut stderr).await;
}

/// Kills a plugin that didn't respond in time and reports it.
async fn timed_out(
    launch: &Launch,
    mut child: Child,
    stderr: &mut Option<Lines<impl AsyncRead + Unpin>>,
    answer: &mut impl FnMut(Answer),
) {
    launch.log(format_args!(
        "didn't respond within {} ms; stopping it",
        launch.timeout.as_millis()
    ));
    let _ = child.start_kill();
    reap(launch, child, stderr).await;
    answer(Answer::Failed(Failure::NoResponse(launch.timeout)));
}

/// Waits up to `timeout_ms` for the process to exit, killing it if it
/// hasn't, and forwards the rest of its stderr. Returns its exit status
/// when it could be read.
async fn reap(
    launch: &Launch,
    mut child: Child,
    stderr: &mut Option<Lines<impl AsyncRead + Unpin>>,
) -> Option<ExitStatus> {
    let deadline = Instant::now() + launch.timeout;
    let status = loop {
        tokio::select! {
            status = child.wait() => break status,
            Some(line) = next_stderr(stderr) => forward(launch, line),
            () = sleep_until(deadline) => {
                launch.log(format_args!(
                    "still running {} ms after answering; stopping it",
                    launch.timeout.as_millis()
                ));
                let _ = child.start_kill();
                break child.wait().await;
            }
        }
    };
    let rest = async {
        while let Some(line) = next_stderr(stderr).await {
            forward(launch, line);
        }
    };
    let _ = timeout(STDERR_GRACE, rest).await;
    match status {
        Ok(status) => Some(status),
        Err(error) => {
            launch.log(format_args!("can't read its exit status: {error}"));
            None
        }
    }
}

/// The next stderr line, or `None` once stderr has ended. Lines too long
/// to keep are replaced by a note.
async fn next_stderr(stderr: &mut Option<Lines<impl AsyncRead + Unpin>>) -> Option<Vec<u8>> {
    let lines = stderr.as_mut()?;
    match lines.next().await {
        Ok(Some(line)) => Some(line),
        Err(FrameError::TooLong) => Some(b"(a line too long to log)".to_vec()),
        Ok(None) | Err(_) => {
            *stderr = None;
            None
        }
    }
}

fn forward(launch: &Launch, line: Vec<u8>) {
    launch.log(String::from_utf8_lossy(&line));
}

/// A response line as read, before its items are checked one by one.
#[derive(Deserialize)]
struct Raw {
    v: u32,
    id: u64,
    #[serde(default)]
    items: Option<Vec<Value>>,
    #[serde(default)]
    result: Option<ActivateResult>,
    #[serde(default)]
    error: Option<ErrorBody>,
    #[serde(default)]
    more: bool,
}

/// What a response line means for `request`, or `None` when it answers
/// another request and is ignored. Anything wrong with it is logged.
fn read_response(launch: &Launch, request: &Request, line: &[u8]) -> Option<Answer> {
    let invalid = |reason: &dyn fmt::Display| {
        launch.log(format_args!("unreadable response: {reason}"));
        Some(Answer::Failed(Failure::Invalid))
    };
    let raw: Raw = match serde_json::from_slice(line) {
        Ok(raw) => raw,
        Err(error) => return invalid(&error),
    };
    if raw.id != request.id {
        launch.log(format_args!(
            "ignoring a response to request {}, not {}",
            raw.id, request.id
        ));
        return None;
    }
    if raw.v != PROTOCOL {
        return invalid(&format_args!(
            "it speaks protocol {}, not {PROTOCOL}; rebuild it against the current SDK",
            raw.v
        ));
    }
    let wants_items = matches!(request.method, Method::List(_) | Method::Query(_));
    match (raw.items, raw.result, raw.error) {
        (None, None, Some(error)) => Some(Answer::Failed(Failure::Plugin(error.message))),
        (Some(items), None, None) if wants_items => Some(Answer::Items {
            items: checked_items(launch, items),
            more: raw.more,
        }),
        (None, Some(result), None) if !wants_items => Some(Answer::Result(result)),
        (None, None, None) => invalid(&"it has none of items, result or error"),
        (Some(_), None, None) | (None, Some(_), None) => invalid(&format_args!(
            "it doesn't answer a {} request",
            method_name(request)
        )),
        _ => invalid(&"it has more than one of items, result or error"),
    }
}

/// The items that follow the rules. Each one dropped is logged with why.
fn checked_items(launch: &Launch, items: Vec<Value>) -> Vec<Item> {
    items
        .into_iter()
        .enumerate()
        .filter_map(|(index, value)| {
            let dropped = |reason: &dyn fmt::Display| {
                launch.log(format_args!("dropping item {index}: {reason}"));
                None
            };
            match serde_json::from_value::<Item>(value) {
                Ok(item) if item.actions.is_empty() => {
                    dropped(&format_args!("{:?} has no actions", item.id))
                }
                Ok(item) => Some(item),
                Err(error) => dropped(&error),
            }
        })
        .collect()
}

fn method_name(request: &Request) -> &'static str {
    match request.method {
        Method::List(_) => "list",
        Method::Query(_) => "query",
        Method::Activate(_) => "activate",
        Method::Cancel(_) => "cancel",
    }
}
