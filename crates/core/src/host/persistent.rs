//! `persistent` plugins (*Lifetimes*, *Transport* and *Errors and timeouts*
//! in `docs/protocol.md`): one process per plugin, started on first use and
//! kept running between requests.
//!
//! Each plugin gets a task that owns its process. Requests and cancels go
//! in through [`Persistent`], and answers come back through the callback
//! given to [`Persistent::start`], tagged with the id the caller gave the
//! request.
//!
//! The process sees its own request ids, counted from 1 by the task, so
//! they're unique per process however the caller numbers its requests, and
//! the task's `cancel` requests get ids of their own.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future;
use std::time::Duration;

use quick_launcher_wire::framing::FrameError;
use quick_launcher_wire::{CancelParams, Method, Request};
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout};

use super::lines::Lines;
use super::{Answer, Failure, Launch, forward, next_stderr, peek, read_response, reap};

/// How many times a plugin that exits is restarted within [`RESTART_WINDOW`].
/// After that it's never started again; every request to it fails with
/// [`Failure::GaveUp`] until the core restarts.
pub const MAX_RESTARTS: usize = 3;

/// See [`MAX_RESTARTS`].
const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// How long a plugin gets to exit after its stdin is closed before it's
/// killed.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(500);

/// How many dropped requests are remembered, to drop their late answers
/// without logging. A plugin may ignore `cancel` and never finish a
/// request, so past this the set is cleared; ids are never reused within a
/// process, so that costs at most a log line per late answer.
const DROPPED_LIMIT: usize = 64;

/// A `persistent` plugin. Dropping it closes the plugin's stdin and lets
/// the plugin exit, as [`Persistent::shutdown`] does, without waiting.
#[derive(Debug)]
pub struct Persistent {
    commands: mpsc::UnboundedSender<Command>,
    task: JoinHandle<()>,
}

#[derive(Debug)]
enum Command {
    Request(Request),
    Cancel(u64),
}

impl Persistent {
    /// The task for the plugin `launch` starts. The process starts with the
    /// first request. Each answer goes to `answer` with the id its request
    /// was given; a request's answers end with one where
    /// [`Answer::is_final`], unless it's cancelled first.
    ///
    /// Needs a tokio runtime.
    pub fn start(launch: Launch, answer: impl FnMut(u64, Answer) + Send + 'static) -> Persistent {
        let (commands, received) = mpsc::unbounded_channel();
        let task = tokio::spawn(Host::new(launch, answer).serve(received));
        Persistent { commands, task }
    }

    /// Sends `request`, starting the process if it isn't running (or
    /// restarting it, within [`MAX_RESTARTS`]). Its answers are reported
    /// with `request.id`, which should be unused among the requests still
    /// waiting. `request` must be one that has a response, so not `cancel`;
    /// that's [`Persistent::cancel`].
    pub fn request(&self, request: Request) {
        debug_assert!(
            !matches!(request.method, Method::Cancel(_)),
            "use Persistent::cancel"
        );
        let _ = self.commands.send(Command::Request(request));
    }

    /// Stops waiting for the request given this id and sends the plugin a
    /// `cancel` for it. Nothing more is reported for it.
    pub fn cancel(&self, request: u64) {
        let _ = self.commands.send(Command::Cancel(request));
    }

    /// Closes the plugin's stdin and waits up to [`SHUTDOWN_GRACE`] for it
    /// to exit, then kills it. The returned handle finishes once the
    /// process is gone. Requests still waiting get no answer.
    ///
    /// A write already in progress finishes first, and a write can take up
    /// to `timeout_ms` when the plugin stops reading stdin, which only a
    /// plugin not built with the SDK does.
    pub fn shutdown(self) -> JoinHandle<()> {
        drop(self.commands);
        self.task
    }
}

/// The running process.
struct Process {
    child: Child,
    /// `None` once a write failed and the process was killed. Its end is
    /// noticed on stdout.
    stdin: Option<ChildStdin>,
    /// The id of the next request written to it.
    next_id: u64,
}

/// A request written to the process and not yet finally answered.
struct Waiting {
    /// The request as the process got it, with the process's id.
    request: Request,
    /// The id the caller gave it, which its answers are reported with.
    tag: u64,
    /// `timeout_ms` after it was written or last answered.
    deadline: Instant,
}

struct Host<F> {
    launch: Launch,
    answer: F,
    process: Option<Process>,
    stdout: Option<Lines<ChildStdout>>,
    stderr: Option<Lines<ChildStderr>>,
    /// By the process's request id.
    waiting: HashMap<u64, Waiting>,
    /// Process request ids cancelled or timed out whose final answer hasn't
    /// arrived. Their answers are dropped without a word.
    dropped: HashSet<u64>,
    /// Whether the process was started successfully before, making the
    /// next start a restart.
    started: bool,
    /// When each restart within the last [`RESTART_WINDOW`] happened.
    restarts: VecDeque<Instant>,
    /// Set once the plugin exited too often.
    given_up: bool,
}

impl<F: FnMut(u64, Answer)> Host<F> {
    fn new(launch: Launch, answer: F) -> Self {
        Host {
            launch,
            answer,
            process: None,
            stdout: None,
            stderr: None,
            waiting: HashMap::new(),
            dropped: HashSet::new(),
            started: false,
            restarts: VecDeque::new(),
            given_up: false,
        }
    }

    async fn serve(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        loop {
            let deadline = self.waiting.values().map(|waiting| waiting.deadline).min();
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Command::Request(request)) => self.request(request).await,
                    Some(Command::Cancel(tag)) => self.cancel(tag).await,
                    None => break,
                },
                line = next_stdout(&mut self.stdout) => self.on_stdout(line).await,
                Some(line) = next_stderr(&self.launch, &mut self.stderr) => {
                    forward(&self.launch, line);
                }
                () = sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                    self.time_out().await;
                }
            }
        }
        self.stop().await;
    }

    async fn request(&mut self, mut request: Request) {
        let tag = request.id;
        if self.process.is_none()
            && let Err(failure) = self.start()
        {
            (self.answer)(tag, Answer::Failed(failure));
            return;
        }
        let process = self.process.as_mut().expect("just started");
        request.id = process.next_id;
        process.next_id += 1;
        let line = encode(&request);
        self.waiting.insert(
            request.id,
            Waiting {
                request,
                tag,
                deadline: Instant::now() + self.launch.timeout,
            },
        );
        // A failed write kills the process, so the request fails when
        // stdout ends, with every other one still waiting.
        self.write(&line).await;
    }

    async fn cancel(&mut self, tag: u64) {
        let Some(id) = self
            .waiting
            .iter()
            .find_map(|(&id, waiting)| (waiting.tag == tag).then_some(id))
        else {
            return;
        };
        self.waiting.remove(&id);
        self.drop_request(id).await;
    }

    /// Forgets request `id` and tells the plugin to stop working on it.
    async fn drop_request(&mut self, id: u64) {
        if self.dropped.len() >= DROPPED_LIMIT {
            self.dropped.clear();
        }
        self.dropped.insert(id);
        let Some(process) = self.process.as_mut() else {
            return;
        };
        let cancel = Request::new(
            process.next_id,
            Method::Cancel(CancelParams { request: id }),
        );
        process.next_id += 1;
        self.write(&encode(&cancel)).await;
    }

    /// Starts the process, unless it exited too often. A start that fails
    /// isn't an exit: until the process has started once, nothing counts
    /// against [`MAX_RESTARTS`].
    fn start(&mut self) -> Result<(), Failure> {
        if self.given_up {
            return Err(Failure::GaveUp);
        }
        if self.started {
            let now = Instant::now();
            while self
                .restarts
                .front()
                .is_some_and(|&restart| now.duration_since(restart) >= RESTART_WINDOW)
            {
                self.restarts.pop_front();
            }
            if self.restarts.len() >= MAX_RESTARTS {
                self.given_up = true;
                self.launch.log(format_args!(
                    "stopped {} times within {} s; it won't be started again \
                     until the launcher restarts",
                    MAX_RESTARTS + 1,
                    RESTART_WINDOW.as_secs()
                ));
                return Err(Failure::GaveUp);
            }
            self.restarts.push_back(now);
            self.launch.log("restarting it");
        }
        let mut child = self.launch.start().map_err(|error| {
            self.launch
                .log(format_args!("can't start {:?}: {error}", self.launch.argv));
            Failure::Start(error.to_string())
        })?;
        self.stdout = Some(Lines::new(child.stdout.take().expect("stdout is piped")));
        self.stderr = Some(Lines::new(child.stderr.take().expect("stderr is piped")));
        self.process = Some(Process {
            stdin: child.stdin.take(),
            child,
            next_id: 1,
        });
        self.started = true;
        Ok(())
    }

    /// Writes one request line, killing the process if that fails or takes
    /// longer than `timeout_ms`.
    async fn write(&mut self, line: &[u8]) {
        let Some(process) = self.process.as_mut() else {
            return;
        };
        let Some(stdin) = process.stdin.as_mut() else {
            return;
        };
        let written = timeout(self.launch.timeout, async {
            stdin.write_all(line).await?;
            stdin.flush().await
        });
        let problem = match written.await {
            Ok(Ok(())) => return,
            Ok(Err(error)) => format!("can't write to it: {error}"),
            Err(_) => format!(
                "didn't read its request within {} ms",
                self.launch.timeout.as_millis()
            ),
        };
        self.launch.log(format_args!("{problem}; stopping it"));
        process.stdin = None;
        let _ = process.child.start_kill();
    }

    async fn on_stdout(&mut self, line: Result<Option<Vec<u8>>, FrameError>) {
        let line = match line {
            Ok(Some(line)) if line.iter().all(u8::is_ascii_whitespace) => return,
            Ok(Some(line)) => line,
            Ok(None) => return self.exited().await,
            Err(FrameError::Io(error)) => {
                self.launch
                    .log(format_args!("can't read its output: {error}; stopping it"));
                if let Some(process) = self.process.as_mut() {
                    let _ = process.child.start_kill();
                }
                return self.exited().await;
            }
            Err(error) => {
                self.launch
                    .log(format_args!("unreadable response: {error}"));
                return self.fail_all(Failure::Invalid);
            }
        };
        let (id, more) = match peek(&line) {
            Ok(peeked) => peeked,
            Err(error) => {
                self.launch
                    .log(format_args!("unreadable response: {error}"));
                return self.fail_all(Failure::Invalid);
            }
        };
        let Some(waiting) = self.waiting.get_mut(&id) else {
            if !self.dropped.contains(&id) {
                self.launch.log(format_args!(
                    "ignoring a response to request {id}, which isn't waiting for one"
                ));
            } else if !more {
                self.dropped.remove(&id);
            }
            return;
        };
        let answer = read_response(&self.launch, &waiting.request, &line)
            .expect("the response's id is the request's");
        let tag = waiting.tag;
        if answer.is_final() {
            self.waiting.remove(&id);
        } else {
            waiting.deadline = Instant::now() + self.launch.timeout;
        }
        (self.answer)(tag, answer);
    }

    /// Fails every request whose deadline has passed. The process keeps
    /// running for the next request.
    async fn time_out(&mut self) {
        let now = Instant::now();
        let expired: Vec<u64> = self
            .waiting
            .iter()
            .filter(|(_, waiting)| waiting.deadline <= now)
            .map(|(&id, _)| id)
            .collect();
        for id in expired {
            let waiting = self.waiting.remove(&id).expect("just found");
            self.launch.log(format_args!(
                "didn't answer request {id} within {} ms; leaving it running",
                self.launch.timeout.as_millis()
            ));
            (self.answer)(
                waiting.tag,
                Answer::Failed(Failure::NoResponse(self.launch.timeout)),
            );
            self.drop_request(id).await;
        }
    }

    /// The process's stdout ended: reaps it, and fails every request still
    /// waiting. The next request restarts it.
    async fn exited(&mut self) {
        self.stdout = None;
        let Some(process) = self.process.take() else {
            return;
        };
        drop(process.stdin);
        let status = reap(&self.launch, process.child, &mut self.stderr).await;
        self.stderr = None;
        self.dropped.clear();
        match status {
            Some(status) => self.launch.log(format_args!("stopped ({status})")),
            None => self.launch.log("stopped"),
        }
        self.fail_all(Failure::Exited(status));
    }

    fn fail_all(&mut self, failure: Failure) {
        let mut waiting: Vec<Waiting> = self.waiting.drain().map(|(_, waiting)| waiting).collect();
        waiting.sort_by_key(|waiting| waiting.request.id);
        for waiting in waiting {
            (self.answer)(waiting.tag, Answer::Failed(failure.clone()));
        }
    }

    /// Closes stdin, then waits up to [`SHUTDOWN_GRACE`] for the process to
    /// exit, reading what it still writes, and kills it if it doesn't.
    async fn stop(&mut self) {
        let Some(Process {
            mut child, stdin, ..
        }) = self.process.take()
        else {
            return;
        };
        drop(stdin);
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        let status = loop {
            tokio::select! {
                status = child.wait() => break status.ok(),
                line = next_stdout(&mut self.stdout) => {
                    // Answers nobody waits for any more; only the end matters.
                    if !matches!(line, Ok(Some(_))) {
                        self.stdout = None;
                    }
                }
                Some(line) = next_stderr(&self.launch, &mut self.stderr) => {
                    forward(&self.launch, line);
                }
                () = sleep_until(deadline) => {
                    self.launch.log(format_args!(
                        "still running {} ms after being told to stop; stopping it",
                        SHUTDOWN_GRACE.as_millis()
                    ));
                    let _ = child.start_kill();
                    break child.wait().await.ok();
                }
            }
        };
        if let Some(status) = status.filter(|status| !status.success()) {
            self.launch.log(format_args!("stopped ({status})"));
        }
    }
}

/// The next stdout line. Never resolves when there's no process.
async fn next_stdout(
    stdout: &mut Option<Lines<ChildStdout>>,
) -> Result<Option<Vec<u8>>, FrameError> {
    match stdout {
        Some(lines) => lines.next().await,
        None => future::pending().await,
    }
}

fn encode(request: &Request) -> Vec<u8> {
    let mut line = serde_json::to_vec(request).expect("requests always encode");
    line.push(b'\n');
    line
}
