//! The launcher's state machine (*Frontend protocol*, *Filtering and
//! ranking* and *Frontend items* in `docs/protocol.md`).
//!
//! The session does no IO of its own. It takes [`Event`]s (frontend
//! messages, plugin answers, the outcome of an action it asked for) and
//! returns [`Effect`]s for the main loop to carry out, so every rule here is
//! a plain unit test.
//!
//! So far it covers the plain search over every global list plugin, modes
//! (entered by prefix, by `open` or by the frontend's `input.mode`) for list
//! and query plugins, and activating actions that run through the platform.
//! Mode hints and query items come in M10, `plugin` actions in M11.

use std::cmp::Ordering;

use quick_launcher_wire::matcher::{self, Score};
use quick_launcher_wire::{After, Item, Method, QueryParams, Request, Run};

use crate::discovery::{Kind, Plugin};
use crate::frontend::{self, FrontendAction, FrontendItem, Incoming, Mode, Outgoing, State};
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
    /// A rescan found these plugins, in discovery order. The session in
    /// progress ends, since its items came from the old ones; the main loop
    /// sends this right before the `open` that triggered the rescan.
    Plugins(Vec<Plugin>),
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

/// One plugin's items in the current session: a global list plugin's, or
/// the active mode's.
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
    /// For a query plugin, the query of the last request sent.
    query: Option<String>,
}

/// The active mode.
#[derive(Debug, Clone, PartialEq)]
struct Active {
    /// Index into the session's plugins.
    plugin: usize,
    /// The prefix the user typed to enter it, as typed, or `None` when it
    /// was entered another way (`open`, or the frontend's `input.mode`).
    prefix: Option<String>,
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
    mode: Option<Active>,
    /// The typed text after the prefix, as typed.
    text: String,
    /// One per global list plugin, plus the active mode's (and a mode's
    /// left this session), in discovery order.
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
            mode: None,
            text: String::new(),
            sources: Vec::new(),
            running: None,
            action_error: None,
            next_request: 1,
        }
    }

    pub fn handle(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Frontend(message) => self.on_message(message),
            Event::Answer { request, answer } => self.on_answer(request, answer),
            Event::Ran(outcome) => self.on_ran(outcome),
            Event::Plugins(plugins) => {
                // Sources and the mode index into the plugins, so they go
                // with them.
                let effects = self.end();
                self.mode = None;
                self.plugins = plugins;
                effects
            }
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
                self.max_items = if max_items == 0 {
                    self.log.write(format_args!(
                        "max_items 0 would leave every state empty; using {}",
                        frontend::DEFAULT_MAX_ITEMS
                    ));
                    frontend::DEFAULT_MAX_ITEMS
                } else {
                    max_items
                };
                vec![ready]
            }
            Incoming::InvalidHello { reason } => {
                // Still a `ready`, so the frontend learns the core's major
                // before it sees EOF.
                self.log
                    .write(format_args!("invalid hello ({reason}); exiting"));
                vec![
                    Effect::Send(Outgoing::Ready {
                        core_version: env!("CARGO_PKG_VERSION").to_owned(),
                    }),
                    Effect::Exit(1),
                ]
            }
            Incoming::Open { mode } => {
                let mut effects = self.end();
                self.text.clear();
                self.mode = mode.and_then(|id| self.mode_named(&id, "opening the plain search"));
                effects.extend(self.refresh());
                effects.push(self.state());
                effects
            }
            Incoming::Input { seq, mode, text } => self.input(seq, mode, text),
            Incoming::LeaveMode => self.leave_mode(),
            // There are no mode hints yet, so there's nothing to complete.
            Incoming::Complete { .. } => Vec::new(),
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
                    self.action_error = None;
                    let mut effects = self.refresh();
                    effects.push(self.state());
                    effects
                }
            },
        }
    }

    /// Reads `input` in the frontend's mode (*Reading input as the user saw
    /// it*): a mode the frontend shows takes `text` literally, while in the
    /// plain search a prefix at the start of `text` enters its mode.
    fn input(&mut self, seq: u64, mode: Option<String>, text: String) -> Vec<Effect> {
        self.seq = seq;
        self.action_error = None;
        let shown = mode.and_then(|id| self.mode_named(&id, "reading the input as plain search"));
        let (mode, text) = match shown {
            Some(Active { plugin, .. }) => {
                // Still the mode the user entered: keep how they entered it.
                let prefix = match &self.mode {
                    Some(active) if active.plugin == plugin => active.prefix.clone(),
                    _ => None,
                };
                (Some(Active { plugin, prefix }), text)
            }
            None => match self.parse_prefix(&text) {
                Some((plugin, len)) => {
                    let prefix = Some(text[..len].to_owned());
                    let rest = text[len..].trim_start().to_owned();
                    (Some(Active { plugin, prefix }), rest)
                }
                None => (None, text),
            },
        };
        let entering = mode.as_ref().map(|active| active.plugin)
            != self.mode.as_ref().map(|active| active.plugin);
        let mut effects = if entering { self.leave() } else { Vec::new() };
        self.mode = mode;
        self.text = text;
        effects.extend(if entering {
            self.enter()
        } else {
            self.query_if_changed()
        });
        effects.push(self.state());
        effects
    }

    /// Leaves the active mode, putting back the prefix that entered it (or
    /// its first prefix), minus its last character, as if the user had
    /// deleted the `:`.
    fn leave_mode(&mut self) -> Vec<Effect> {
        let Some(active) = self.mode.clone() else {
            return Vec::new();
        };
        let prefix = active
            .prefix
            .or_else(|| self.plugins[active.plugin].prefixes.first().cloned())
            .unwrap_or_default();
        let mut kept = prefix.chars();
        kept.next_back();
        let mut effects = self.leave();
        self.mode = None;
        self.text = format!("{}{}", kept.as_str(), self.text);
        self.action_error = None;
        effects.push(self.state());
        effects
    }

    /// The mode for the plugin `id`, entered without a prefix. An unknown
    /// id is logged, saying what happens instead.
    fn mode_named(&self, id: &str, instead: &str) -> Option<Active> {
        let plugin = self.plugins.iter().position(|plugin| plugin.id() == id);
        if plugin.is_none() {
            self.log
                .write(format_args!("there is no mode {id:?}; {instead}"));
        }
        plugin.map(|plugin| Active {
            plugin,
            prefix: None,
        })
    }

    /// The plugin whose prefix starts `text`, ignoring case, and the
    /// prefix's length in `text`. The longest matching prefix wins.
    fn parse_prefix(&self, text: &str) -> Option<(usize, usize)> {
        let mut best: Option<(usize, usize, usize)> = None;
        for (index, plugin) in self.plugins.iter().enumerate() {
            for prefix in &plugin.prefixes {
                let Some(len) = prefix_len(text, prefix) else {
                    continue;
                };
                let chars = prefix.chars().count();
                if best.is_none_or(|(_, _, longest)| chars > longest) {
                    best = Some((index, len, chars));
                }
            }
        }
        best.map(|(plugin, len, _)| (plugin, len))
    }

    /// Stops waiting on the active mode's plugin, unless the plain search
    /// still needs its items. Its items are kept for when it's entered
    /// again.
    fn leave(&mut self) -> Vec<Effect> {
        let Some(active) = &self.mode else {
            return Vec::new();
        };
        if is_global(&self.plugins[active.plugin]) {
            return Vec::new();
        }
        let plugin = active.plugin;
        self.sources
            .iter_mut()
            .filter(|source| source.plugin == plugin)
            .filter_map(|source| {
                source.partial = false;
                source.pending.take().map(Effect::Cancel)
            })
            .collect()
    }

    /// Asks the active mode's plugin for its items: a list plugin's list
    /// (unless a request for it is already out), a query plugin's items for
    /// the query.
    fn enter(&mut self) -> Vec<Effect> {
        let Some(active) = &self.mode else {
            return Vec::new();
        };
        let plugin = active.plugin;
        match self.plugins[plugin].manifest.kind {
            Kind::List => {
                let pending = self
                    .source(plugin)
                    .is_some_and(|source| source.pending.is_some());
                if pending {
                    Vec::new()
                } else {
                    self.request(plugin, Method::list())
                }
            }
            Kind::Query => self.request(plugin, self.query_method()),
        }
    }

    /// In a query mode, sends a `query` request unless the last one was for
    /// the same query.
    fn query_if_changed(&mut self) -> Vec<Effect> {
        let Some(active) = &self.mode else {
            return Vec::new();
        };
        let plugin = active.plugin;
        if self.plugins[plugin].manifest.kind != Kind::Query {
            return Vec::new();
        }
        let asked = self
            .source(plugin)
            .and_then(|source| source.query.as_deref());
        if asked == Some(self.text.trim()) {
            return Vec::new();
        }
        self.request(plugin, self.query_method())
    }

    fn query_method(&self) -> Method {
        Method::Query(QueryParams {
            text: self.text.trim().to_owned(),
        })
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

    /// The plugin and item a frontend `ref` names, if the current state
    /// shows that item. An item the session holds but doesn't show (the
    /// newest text filtered it out) no longer resolves.
    fn resolve(&self, item_ref: &str) -> Option<(&Plugin, &Item)> {
        let (plugin_id, item_id) = item_ref.split_once('/')?;
        self.visible().into_iter().find_map(|(source, index)| {
            let source = &self.sources[source];
            let plugin = &self.plugins[source.plugin];
            let item = &source.items[index];
            (plugin.id() == plugin_id && item.id == item_id).then_some((plugin, item))
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

    /// Asks every global list plugin and the active mode's plugin for
    /// their items, keeping what each one gave before until it answers.
    fn refresh(&mut self) -> Vec<Effect> {
        let mode = self.mode.as_ref().map(|active| active.plugin);
        let mut effects = Vec::new();
        for plugin in 0..self.plugins.len() {
            if is_global(&self.plugins[plugin]) {
                effects.extend(self.request(plugin, Method::list()));
            } else if Some(plugin) == mode {
                effects.extend(match self.plugins[plugin].manifest.kind {
                    Kind::List => self.request(plugin, Method::list()),
                    Kind::Query => self.request(plugin, self.query_method()),
                });
            }
        }
        effects
    }

    /// Sends `method` to `plugin`, cancelling its outstanding request. Its
    /// items stay until it answers.
    fn request(&mut self, plugin: usize, method: Method) -> Vec<Effect> {
        let id = self.next_request;
        self.next_request += 1;
        let index = match self
            .sources
            .binary_search_by_key(&plugin, |source| source.plugin)
        {
            Ok(index) => index,
            Err(index) => {
                self.sources.insert(
                    index,
                    Source {
                        plugin,
                        items: Vec::new(),
                        pending: None,
                        partial: false,
                        error: None,
                        query: None,
                    },
                );
                index
            }
        };
        let source = &mut self.sources[index];
        let mut effects: Vec<Effect> = source
            .pending
            .replace(id)
            .map(Effect::Cancel)
            .into_iter()
            .collect();
        source.partial = false;
        source.error = None;
        source.query = match &method {
            Method::Query(params) => Some(params.text.clone()),
            _ => None,
        };
        effects.push(Effect::Request {
            plugin: self.plugins[plugin].clone(),
            request: Request::new(id, method),
        });
        effects
    }

    fn source(&self, plugin: usize) -> Option<&Source> {
        self.sources.iter().find(|source| source.plugin == plugin)
    }

    /// The sources the current state draws from, as indices into
    /// `sources`: the active mode's, or in the plain search every global
    /// plugin's.
    fn shown(&self) -> Vec<usize> {
        (0..self.sources.len())
            .filter(|&index| {
                let plugin = self.sources[index].plugin;
                match &self.mode {
                    Some(active) => plugin == active.plugin,
                    None => is_global(&self.plugins[plugin]),
                }
            })
            .collect()
    }

    /// The items the current state shows, best first and at most
    /// `max_items`, as (index into `sources`, index into its items).
    fn visible(&self) -> Vec<(usize, usize)> {
        let query = self.text.trim();
        let shown = self.shown();
        let query_mode = self
            .mode
            .as_ref()
            .is_some_and(|active| self.plugins[active.plugin].manifest.kind == Kind::Query);
        let mut visible: Vec<(usize, usize)> = if query_mode {
            // Shown as given, sorted by score when present.
            let mut visible: Vec<(usize, usize)> = shown
                .iter()
                .flat_map(|&source| {
                    (0..self.sources[source].items.len()).map(move |index| (source, index))
                })
                .collect();
            let score = |&(source, index): &(usize, usize)| self.sources[source].items[index].score;
            visible.sort_by(|a, b| by_score(score(a), score(b)));
            visible
        } else {
            let mut matches: Vec<(Score, usize, usize)> = shown
                .iter()
                .flat_map(|&source| {
                    matcher::filter(&self.sources[source].items, query)
                        .into_iter()
                        .map(move |m| (m.score, source, m.index))
                })
                .collect();
            // Match quality, then discovery order, then each plugin's order.
            matches.sort();
            matches
                .into_iter()
                .map(|(_, source, index)| (source, index))
                .collect()
        };
        visible.truncate(self.max_items);
        visible
    }

    /// The state to send now.
    fn state(&self) -> Effect {
        let items = self
            .visible()
            .into_iter()
            .map(|(source, index)| {
                let source = &self.sources[source];
                frontend_item(&self.plugins[source.plugin], &source.items[index])
            })
            .collect();
        let shown = self.shown();
        let error = self.action_error.clone().or_else(|| {
            shown.iter().find_map(|&source| {
                let source = &self.sources[source];
                let failure = source.error.as_ref()?;
                Some(failure_message(&self.plugins[source.plugin], failure))
            })
        });
        let mode = self.mode.as_ref().map(|active| {
            let manifest = &self.plugins[active.plugin].manifest;
            Mode {
                id: manifest.id.clone(),
                name: manifest.name.clone(),
                icon: manifest.icon.clone(),
                placeholder: manifest.placeholder.clone(),
            }
        });
        Effect::Send(Outgoing::State(State {
            seq: self.seq,
            mode,
            text: self.text.clone(),
            loading: shown
                .iter()
                .any(|&source| self.sources[source].pending.is_some()),
            error,
            items,
        }))
    }
}

/// Whether the plain search shows `plugin`'s items.
fn is_global(plugin: &Plugin) -> bool {
    plugin.manifest.kind == Kind::List && plugin.manifest.global
}

/// The length in bytes of `prefix` at the start of `text`, comparing
/// characters without case.
fn prefix_len(text: &str, prefix: &str) -> Option<usize> {
    if prefix.is_empty() {
        return None;
    }
    let mut rest = text.char_indices();
    for expected in prefix.chars() {
        let (_, found) = rest.next()?;
        if !found.to_lowercase().eq(expected.to_lowercase()) {
            return None;
        }
    }
    Some(rest.next().map_or(text.len(), |(index, _)| index))
}

/// Higher scores first; items without one after every item with one.
fn by_score(a: Option<f64>, b: Option<f64>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => b.partial_cmp(&a).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
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
