//! The launcher's state machine (*Frontend protocol*, *Filtering and
//! ranking* and *Frontend items* in `docs/protocol.md`).
//!
//! The session does no IO of its own. It takes [`Event`]s (frontend
//! messages, plugin answers, the outcome of an action it asked for) and
//! returns [`Effect`]s for the main loop to carry out, so every rule here is
//! a plain unit test.
//!
//! This is its first version (M8): the plain search over every global list
//! plugin, and activating actions that run through the platform. Modes come
//! in M9, mode hints and query items in M10, `plugin` actions in M11.

use quick_launcher_wire::matcher::{self, Score};
use quick_launcher_wire::{After, Item, Method, Request, Run};

use crate::discovery::{Kind, Plugin};
use crate::frontend::{self, FrontendAction, FrontendItem, Incoming, Outgoing, State};
use crate::host::{Answer, Failure};
use crate::log::Log;

/// Something that happened, for the session to react to.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A message from the frontend.
    Frontend(Incoming),
    /// A plugin answered the request with this id.
    Answer { request: u64, answer: Answer },
    /// The action the last [`Effect::Run`] asked for ran, or couldn't.
    Ran(Result<(), String>),
}

/// Something for the main loop to do, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Write a message to the frontend.
    Send(Outgoing),
    /// Send `request` to `plugin`. Its answers come back as
    /// [`Event::Answer`] with the request's id.
    Request { plugin: Plugin, request: Request },
    /// Stop the request with this id; its answers are no longer wanted.
    Cancel(u64),
    /// Run an action through the platform, then report the outcome as
    /// [`Event::Ran`].
    Run { plugin: Plugin, run: Run },
    /// Exit the core with this status.
    Exit(i32),
}

/// One global list plugin's items in the current session.
#[derive(Debug, Clone)]
struct Source {
    /// Index into the session's plugins.
    plugin: usize,
    items: Vec<Item>,
    /// The outstanding request, if any.
    pending: Option<u64>,
    /// Whether `items` came from a partial answer to `pending`.
    partial: bool,
    error: Option<Failure>,
}

/// An action that is running, waiting for [`Event::Ran`].
#[derive(Debug, Clone)]
struct Running {
    title: String,
    after: After,
}

/// The launcher's state between frontend messages.
#[derive(Debug)]
pub struct Session {
    log: Log,
    /// Enabled plugins in discovery order, as of the last scan.
    plugins: Vec<Plugin>,
    max_items: usize,
    seq: u64,
    text: String,
    /// One per global list plugin, in discovery order.
    sources: Vec<Source>,
    running: Option<Running>,
    /// Why the last action couldn't run, until the next input.
    action_error: Option<String>,
    next_request: u64,
}

impl Session {
    pub fn new(plugins: Vec<Plugin>, log: Log) -> Session {
        Session {
            log,
            plugins,
            max_items: frontend::DEFAULT_MAX_ITEMS,
            seq: 0,
            text: String::new(),
            sources: Vec::new(),
            running: None,
            action_error: None,
            next_request: 1,
        }
    }

    /// Replaces the plugins after a rescan. Takes effect on the next
    /// `open`, which the main loop sends right after.
    pub fn set_plugins(&mut self, plugins: Vec<Plugin>) {
        self.plugins = plugins;
    }

    pub fn handle(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Frontend(message) => self.on_message(message),
            Event::Answer { request, answer } => self.on_answer(request, answer),
            Event::Ran(outcome) => self.on_ran(outcome),
        }
    }

    fn on_message(&mut self, message: Incoming) -> Vec<Effect> {
        match message {
            Incoming::Hello {
                v,
                client,
                client_version,
                max_items,
            } => {
                let ready = Effect::Send(Outgoing::Ready {
                    core_version: env!("CARGO_PKG_VERSION").to_owned(),
                });
                if v != frontend::VERSION {
                    self.log.write(format_args!(
                        "{client} {client_version} speaks frontend protocol {v}, \
                         but this core speaks {}; exiting",
                        frontend::VERSION
                    ));
                    return vec![ready, Effect::Exit(1)];
                }
                self.log
                    .write(format_args!("frontend: {client} {client_version}"));
                self.max_items = max_items;
                vec![ready]
            }
            Incoming::Open { mode } => {
                if let Some(mode) = mode {
                    self.log.write(format_args!(
                        "can't open straight into mode {mode:?} yet; opening the plain search"
                    ));
                }
                let mut effects = self.end();
                self.text.clear();
                effects.extend(self.request_items());
                effects.push(self.state());
                effects
            }
            Incoming::Input { seq, mode, text } => {
                if let Some(mode) = mode {
                    self.log.write(format_args!(
                        "modes aren't supported yet; reading input for {mode:?} as plain search"
                    ));
                }
                self.seq = seq;
                self.text = text;
                self.action_error = None;
                vec![self.state()]
            }
            // No mode is ever active and there are no mode hints yet, so
            // both do nothing.
            Incoming::LeaveMode | Incoming::Complete { .. } => Vec::new(),
            Incoming::Activate { item, action } => self.activate(&item, &action),
            Incoming::Close => self.end(),
        }
    }

    fn on_answer(&mut self, request: u64, answer: Answer) -> Vec<Effect> {
        let Some(source) = self
            .sources
            .iter_mut()
            .find(|source| source.pending == Some(request))
        else {
            // An answer to a request from an earlier session.
            return Vec::new();
        };
        match answer {
            Answer::Items { items, more } => {
                source.items = items;
                source.error = None;
                source.partial = more;
                if !more {
                    source.pending = None;
                }
            }
            Answer::Result(_) => {
                // The host never passes a result as an answer to `list`.
                source.pending = None;
            }
            Answer::Failed(failure) => {
                // A timeout after a partial answer leaves its items in place;
                // any other failure means no items from this plugin.
                if !(source.partial && matches!(failure, Failure::NoResponse(_))) {
                    source.items.clear();
                }
                source.error = Some(failure);
                source.pending = None;
                source.partial = false;
            }
        }
        vec![self.state()]
    }

    fn on_ran(&mut self, outcome: Result<(), String>) -> Vec<Effect> {
        let Some(running) = self.running.take() else {
            return Vec::new();
        };
        match outcome {
            Err(error) => {
                self.log
                    .write(format_args!("couldn't run {:?}: {error}", running.title));
                self.action_error = Some(format!("Couldn't run “{}”: {error}", running.title));
                vec![self.state()]
            }
            Ok(()) => match running.after {
                After::Close => {
                    let mut effects = self.end();
                    effects.push(Effect::Send(Outgoing::Done));
                    effects
                }
                After::Refresh => {
                    let mut effects = self.cancel_requests();
                    effects.extend(self.request_items());
                    effects.push(self.state());
                    effects
                }
            },
        }
    }

    fn activate(&mut self, item_ref: &str, action_id: &str) -> Vec<Effect> {
        let Some((plugin, item)) = self.resolve(item_ref) else {
            self.log.write(format_args!(
                "can't activate {item_ref:?}: no such item; sending the current state"
            ));
            return vec![self.state()];
        };
        let Some((index, action)) = item
            .actions
            .iter()
            .enumerate()
            .find(|(index, action)| action_key(action, *index) == action_id)
        else {
            self.log.write(format_args!(
                "can't activate {item_ref:?}: it has no action {action_id:?}"
            ));
            return vec![self.state()];
        };
        if item.disabled && index == 0 {
            self.log.write(format_args!(
                "not running {item_ref:?}'s first action: the item is disabled"
            ));
            return vec![self.state()];
        }
        if let Run::Plugin(_) = action.run {
            self.log.write(format_args!(
                "not running {item_ref:?}'s action {action_id:?}: \
                 plugin actions aren't supported yet"
            ));
            return vec![self.state()];
        }
        let effect = Effect::Run {
            plugin: plugin.clone(),
            run: action.run.clone(),
        };
        self.running = Some(Running {
            title: action.title.clone(),
            after: action.after,
        });
        vec![effect]
    }

    /// The plugin and item a frontend `ref` names, if the session still
    /// holds that item.
    fn resolve(&self, item_ref: &str) -> Option<(&Plugin, &Item)> {
        let (plugin_id, item_id) = item_ref.split_once('/')?;
        self.sources.iter().find_map(|source| {
            let plugin = &self.plugins[source.plugin];
            if plugin.id() != plugin_id {
                return None;
            }
            let item = source.items.iter().find(|item| item.id == item_id)?;
            Some((plugin, item))
        })
    }

    /// Ends the session: outstanding requests are cancelled and the
    /// items forgotten. The text and `seq` stay until the next `open`.
    fn end(&mut self) -> Vec<Effect> {
        let effects = self.cancel_requests();
        self.sources.clear();
        self.running = None;
        self.action_error = None;
        effects
    }

    fn cancel_requests(&mut self) -> Vec<Effect> {
        self.sources
            .iter_mut()
            .filter_map(|source| {
                source.partial = false;
                source.pending.take().map(Effect::Cancel)
            })
            .collect()
    }

    /// Asks every global list plugin for its items, keeping what each one
    /// gave before until it answers.
    fn request_items(&mut self) -> Vec<Effect> {
        let globals: Vec<usize> = self
            .plugins
            .iter()
            .enumerate()
            .filter(|(_, plugin)| plugin.manifest.kind == Kind::List && plugin.manifest.global)
            .map(|(index, _)| index)
            .collect();
        let mut effects = Vec::new();
        let mut sources = Vec::new();
        for plugin in globals {
            let id = self.next_request;
            self.next_request += 1;
            let mut source = self
                .sources
                .iter()
                .find(|source| source.plugin == plugin)
                .cloned()
                .unwrap_or(Source {
                    plugin,
                    items: Vec::new(),
                    pending: None,
                    partial: false,
                    error: None,
                });
            source.pending = Some(id);
            sources.push(source);
            effects.push(Effect::Request {
                plugin: self.plugins[plugin].clone(),
                request: Request::new(id, Method::list()),
            });
        }
        self.sources = sources;
        effects
    }

    /// The state to send now.
    fn state(&self) -> Effect {
        let query = self.text.trim();
        let mut matches: Vec<(Score, usize, usize)> = self
            .sources
            .iter()
            .enumerate()
            .flat_map(|(source, s)| {
                matcher::filter(&s.items, query)
                    .into_iter()
                    .map(move |m| (m.score, source, m.index))
            })
            .collect();
        // Match quality, then discovery order, then each plugin's order.
        matches.sort();
        matches.truncate(self.max_items);
        let items = matches
            .into_iter()
            .map(|(_, source, index)| {
                let source = &self.sources[source];
                frontend_item(&self.plugins[source.plugin], &source.items[index])
            })
            .collect();
        let error = self.action_error.clone().or_else(|| {
            self.sources.iter().find_map(|source| {
                let failure = source.error.as_ref()?;
                Some(failure_message(&self.plugins[source.plugin], failure))
            })
        });
        Effect::Send(Outgoing::State(State {
            seq: self.seq,
            mode: None,
            text: self.text.clone(),
            loading: self.sources.iter().any(|source| source.pending.is_some()),
            error,
            items,
        }))
    }
}

/// An action's id as the frontend sees it: its own, or its index.
fn action_key(action: &quick_launcher_wire::Action, index: usize) -> String {
    action.id.clone().unwrap_or_else(|| index.to_string())
}

/// `item` as the frontend draws it: a `ref` from the plugin id and item id,
/// the icon resolved, and nothing about how its actions run.
fn frontend_item(plugin: &Plugin, item: &Item) -> FrontendItem {
    FrontendItem {
        item_ref: format!("{}/{}", plugin.id(), item.id),
        label: item.label.clone(),
        detail: item.detail.clone(),
        icon: item.icon.clone().or_else(|| plugin.manifest.icon.clone()),
        badge: item.badge.clone(),
        disabled: item.disabled,
        actions: item
            .actions
            .iter()
            .enumerate()
            .map(|(index, action)| FrontendAction {
                id: action_key(action, index),
                title: action.title.clone(),
                key: action.key.clone(),
            })
            .collect(),
    }
}

/// A failure as the state's one-line `error`, naming the plugin.
fn failure_message(plugin: &Plugin, failure: &Failure) -> String {
    let name = &plugin.manifest.name;
    match failure {
        Failure::Plugin(message) => format!("{name}: {message}"),
        failure => format!("{name} {failure}"),
    }
}

#[cfg(test)]
mod tests;
