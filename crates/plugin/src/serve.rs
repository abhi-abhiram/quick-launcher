//! The process side of a plugin: reading requests, calling the [`Plugin`] and
//! writing its answers (*Transport*, *Requests* and *Responses* in
//! `docs/protocol.md`).
//!
//! A reader thread reads stdin and hands requests to the thread running the
//! plugin, one at a time. It handles `cancel` itself, so a running `query`
//! sees it at once.

use std::any::Any;
use std::collections::HashMap;
use std::io::{self, BufReader, Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::anyhow;
use quick_launcher_wire::framing::{self, FrameError};
use quick_launcher_wire::{
    ActivateParams, ErrorBody, Method, PROTOCOL, Request, Response, ResponseBody,
};

use crate::{Item, Outcome};

/// A plugin: its own logic, with the process side left to [`run`].
///
/// Implement the methods the plugin's kind needs: `list` for a `list`
/// plugin, `query` for a `query` plugin, and `activate` if any of its
/// actions are `plugin` actions. The others answer with an error.
///
/// The `pi` plugin from *Plugin SDK* in `docs/protocol.md`:
///
/// ```no_run
/// use quick_launcher_plugin::{Item, Action, Run, Activate, Outcome, Plugin};
/// # fn projects() -> anyhow::Result<Vec<String>> { Ok(vec![]) }
/// # fn name(path: &str) -> &str { path }
/// # fn tilde(path: &str) -> String { path.into() }
/// # fn toggle_pin(item: &str) -> anyhow::Result<()> { Ok(()) }
///
/// struct Pi;
///
/// impl Plugin for Pi {
///     fn list(&mut self) -> anyhow::Result<Vec<Item>> {
///         Ok(projects()?.into_iter().map(|path| {
///             Item::new(&path, name(&path))
///                 .detail(tilde(&path))
///                 .action(Action::new("open", "Open pi").run(Run::terminal(["pi"]).cwd(&path)))
///                 .action(Action::new("pin", "Pin").plugin().key("ctrl+p"))
///         }).collect())
///     }
///
///     fn activate(&mut self, a: Activate) -> anyhow::Result<Outcome> {
///         if let Some(item) = &a.item {
///             toggle_pin(item)?;
///         }
///         Ok(Outcome::refresh())
///     }
/// }
///
/// fn main() { quick_launcher_plugin::run(Pi) }
/// ```
pub trait Plugin {
    /// Every item, for a `list` plugin. The core filters and ranks them, and
    /// keeps this order for ties.
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        Err(anyhow!("this plugin has no list"))
    }

    /// Items for the typed text, for a `query` plugin. They're shown as
    /// given, sorted by [`Item::score`] when set.
    fn query(&mut self, query: &Query) -> anyhow::Result<Vec<Item>> {
        let _ = query;
        Err(anyhow!("this plugin has no query"))
    }

    /// Runs a `plugin` action the user chose.
    fn activate(&mut self, activate: Activate) -> anyhow::Result<Outcome> {
        let _ = activate;
        Err(anyhow!("this plugin has no actions to run"))
    }
}

/// A `plugin` action the user chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activate {
    /// The item's id, or `None` for the manifest's `[query_item]`, whose
    /// actions use [`text`](Activate::text) instead.
    pub item: Option<String>,
    /// The action's id.
    pub action: String,
    /// The query when the action was chosen, or for the `[query_item]` the
    /// text it was built from.
    pub text: String,
}

/// A `query` request in progress.
pub struct Query {
    id: u64,
    text: String,
    cancelled: Arc<AtomicBool>,
    out: Output,
}

impl Query {
    /// The query: what follows the prefix, trimmed.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Sends `items` as a partial answer, which replaces any sent before. The
    /// list `query` returns is the final one. Use it so a slow answer (an AI
    /// reply, a network search) shows while it's still coming.
    pub fn partial(&self, items: Vec<Item>) -> anyhow::Result<()> {
        let mut response = Response::new(self.id, items_body(items));
        response.more = true;
        self.out.send(&response)?;
        Ok(())
    }

    /// Whether the core has cancelled this request because the text changed.
    /// Nobody will see the answer, so a query doing expensive work should
    /// check this and stop. What it returns then is dropped.
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// Answers requests from stdin on stdout until stdin closes, then returns.
/// The same binary works as a `oneshot` plugin, which gets one request, and
/// a `persistent` one.
///
/// Only protocol lines go to stdout, so the plugin itself must not print
/// there; stderr is for logs. Exits with status 1 if stdout can't be written.
pub fn run<P: Plugin>(plugin: P) {
    if let Err(e) = run_on(plugin, io::stdin(), io::stdout()) {
        eprintln!("quick-launcher-plugin: {e}");
        std::process::exit(1);
    }
}

/// [`run`] over any input and output, such as pipes in a test. Returns at
/// the end of `input`, or with the error if `output` fails.
pub fn run_on<P, R, W>(mut plugin: P, input: R, output: W) -> io::Result<()>
where
    P: Plugin,
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let out = Output(Arc::new(Mutex::new(Box::new(output))));
    let running: Running = Arc::default();
    let (requests, received) = mpsc::channel();
    {
        let (out, running) = (out.clone(), running.clone());
        thread::spawn(move || read_requests(input, requests, out, running));
    }
    for (request, cancelled) in received {
        let id = request.id;
        let body = answer(&mut plugin, request.method, id, cancelled, &out);
        running.lock().unwrap().remove(&id);
        match out.send(&Response::new(id, body)) {
            Ok(()) => {}
            Err(FrameError::Io(e)) => return Err(e),
            Err(e) => {
                let message = format!("the plugin's answer can't be sent: {e}");
                out.send(&Response::new(id, error_body(message)))
                    .map_err(io::Error::other)?;
            }
        }
    }
    Ok(())
}

/// Each request read but not yet answered, by id, with its cancel flag.
type Running = Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>;

/// Reads requests until EOF, passing them on with a cancel flag. Acts on
/// `cancel` here, and answers what can't be passed on (another protocol
/// version) itself.
fn read_requests(
    input: impl Read,
    requests: mpsc::Sender<(Request, Arc<AtomicBool>)>,
    out: Output,
    running: Running,
) {
    let mut input = BufReader::new(input);
    let mut buf = Vec::new();
    loop {
        let request: Request = match framing::read_message(&mut input, &mut buf) {
            Ok(Some(request)) => request,
            Ok(None) => return,
            Err(FrameError::Io(e)) => {
                eprintln!("quick-launcher-plugin: reading requests: {e}");
                return;
            }
            // There's no id to answer, so all we can do is log it.
            Err(e) => {
                eprintln!("quick-launcher-plugin: skipping a request: {e}");
                continue;
            }
        };
        if request.v != PROTOCOL {
            let message = format!(
                "this plugin speaks protocol {PROTOCOL}, not {}; rebuild it against the current SDK",
                request.v
            );
            if out
                .send(&Response::new(request.id, error_body(message)))
                .is_err()
            {
                return;
            }
            continue;
        }
        if let Method::Cancel(params) = &request.method {
            if let Some(flag) = running.lock().unwrap().get(&params.request) {
                flag.store(true, Ordering::Relaxed);
            }
            continue;
        }
        let flag = Arc::new(AtomicBool::new(false));
        running.lock().unwrap().insert(request.id, flag.clone());
        if requests.send((request, flag)).is_err() {
            return;
        }
    }
}

/// Calls the plugin for one request and returns its final answer. A panic
/// becomes an error, so it costs one request and not the process.
fn answer<P: Plugin>(
    plugin: &mut P,
    method: Method,
    id: u64,
    cancelled: Arc<AtomicBool>,
    out: &Output,
) -> ResponseBody {
    let result = panic::catch_unwind(AssertUnwindSafe(|| match method {
        Method::List(_) => plugin.list().map(items_body),
        Method::Query(params) => {
            let query = Query {
                id,
                text: params.text,
                cancelled,
                out: out.clone(),
            };
            plugin.query(&query).map(items_body)
        }
        Method::Activate(ActivateParams { item, action, text }) => plugin
            .activate(Activate { item, action, text })
            .map(|outcome| ResponseBody::Result(outcome.into_wire())),
        // Handled by the reader, which never passes it on.
        Method::Cancel(_) => unreachable!("cancel is handled when read"),
    }));
    match result {
        Ok(Ok(body)) => body,
        Ok(Err(e)) => error_body(format!("{e:#}")),
        Err(payload) => error_body(format!("the plugin crashed: {}", panic_message(&payload))),
    }
}

fn items_body(items: Vec<Item>) -> ResponseBody {
    ResponseBody::Items(items.into_iter().map(Item::into_wire).collect())
}

fn error_body(message: String) -> ResponseBody {
    ResponseBody::Error(ErrorBody { message })
}

fn panic_message(payload: &Box<dyn Any + Send>) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("panic")
}

/// stdout, shared by the plugin thread and the reader, one whole line at a
/// time.
#[derive(Clone)]
struct Output(Arc<Mutex<Box<dyn Write + Send>>>);

impl Output {
    fn send(&self, response: &Response) -> Result<(), FrameError> {
        let mut out = self.0.lock().unwrap_or_else(|e| e.into_inner());
        framing::write_message(&mut *out, response)
    }
}
