//! The launcher's state machine (*Frontend protocol*, *Filtering and
//! ranking* and *Frontend items* in `docs/protocol.md`).
//!
//! The session does no IO of its own. It takes [`Event`]s (frontend
//! messages, plugin answers, the outcome of an action it asked for) and
//! returns [`Effect`]s for the main loop to carry out, so every rule here is
//! a plain unit test.
//!
//! So far it covers the plain search over every global list plugin and the
//! mode hints, modes (entered by prefix, by `open`, by the frontend's
//! `input.mode`, by `complete` or by a hint) for list and query plugins,
//! `[query_item]`s, and activating actions that run through the platform.
//! `plugin` actions come in M11.

use std::cmp::Ordering;

use quick_launcher_wire::matcher::{self, Score};
use quick_launcher_wire::{Action, After, Icon, Item, Method, QueryParams, Request, Run};

use crate::discovery::{Kind, Plugin, QueryItem};
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

/// One row of the state, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    /// A plugin's item: an index into `sources`, and one into its items.
    Item(usize, usize),
    /// The hint for entering this plugin's mode (an index into the
    /// plugins).
    Hint(usize),
    /// The active mode's `[query_item]`, filled in with the query.
    QueryItem,
}

/// What a frontend `ref` names. A plugin item's is `<plugin id>/<item id>`.
/// The core's own rows put a `:` after the plugin id instead, which no
/// plugin id contains, so their refs never equal an item's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ref<'a> {
    Item {
        plugin: &'a str,
        item: &'a str,
    },
    /// `<plugin id>:mode`.
    Hint {
        plugin: &'a str,
    },
    /// `<plugin id>:query:<text>`, recording the query it was built from.
    QueryItem {
        plugin: &'a str,
        text: &'a str,
    },
}

impl<'a> Ref<'a> {
    fn parse(item_ref: &'a str) -> Option<Ref<'a>> {
        let end = item_ref.find(['/', ':'])?;
        let (plugin, rest) = (&item_ref[..end], &item_ref[end + 1..]);
        match (item_ref.as_bytes()[end], rest) {
            (b'/', item) => Some(Ref::Item { plugin, item }),
            (_, "mode") => Some(Ref::Hint { plugin }),
            (_, rest) => Some(Ref::QueryItem {
                plugin,
                text: rest.strip_prefix("query:")?,
            }),
        }
    }
}

/// What an `activate` names: a mode hint, or an item to run an action of
/// (a plugin's, or a `[query_item]` filled in with its recorded text).
enum Target {
    Hint(usize),
    Item(usize, Item),
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
    /// left this session). Sorted by `plugin`, i.e. in discovery order,
    /// which [`Session::source_index`] relies on.
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
                effects.extend(self.ask_all());
                effects.push(self.state());
                effects
            }
            Incoming::Input { seq, mode, text } => self.input(seq, mode, text),
            Incoming::LeaveMode => self.leave_mode(),
            Incoming::Complete { item } => self.complete(item.as_deref()),
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
                    let mut effects = self.ask_shown();
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
        self.switch(mode, text)
    }

    /// Makes `mode` the active mode with `text`, asking its plugin for
    /// items when it's entered or its query changed, and sends the state.
    fn switch(&mut self, mode: Option<Active>, text: String) -> Vec<Effect> {
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

    /// Enters `plugin`'s mode with empty text, as `complete` and a hint's
    /// `enter` action do. `seq` stays, so the state is current unless the
    /// user typed after it.
    fn enter_mode(&mut self, plugin: usize) -> Vec<Effect> {
        self.action_error = None;
        self.switch(
            Some(Active {
                plugin,
                prefix: None,
            }),
            String::new(),
        )
    }

    /// Enters the mode of the hint `item` names, or else, in the plain
    /// search, the first hint's. A hint's ref is honoured even when the
    /// core has since entered a mode: the user chose it on the screen they
    /// saw (*Reading input as the user saw it*).
    fn complete(&mut self, item: Option<&str>) -> Vec<Effect> {
        let selected = match item.and_then(Ref::parse) {
            Some(Ref::Hint { plugin }) => self.plugin_index(plugin),
            _ => None,
        };
        let first = || {
            if self.mode.is_some() {
                return None;
            }
            self.visible(&self.shown())
                .into_iter()
                .find_map(|row| match row {
                    Row::Hint(plugin) => Some(plugin),
                    _ => None,
                })
        };
        match selected.or_else(first) {
            Some(plugin) => self.enter_mode(plugin),
            None => {
                let named = match item {
                    Some(item) => format!("{item:?} names no mode hint"),
                    None => "no item is selected".to_owned(),
                };
                self.log.write(format_args!(
                    "nothing to complete: {named}, and the state shows no hint"
                ));
                Vec::new()
            }
        }
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
        let plugin = self.plugin_index(id);
        if plugin.is_none() {
            self.log
                .write(format_args!("there is no mode {id:?}; {instead}"));
        }
        plugin.map(|plugin| Active {
            plugin,
            prefix: None,
        })
    }

    fn plugin_index(&self, id: &str) -> Option<usize> {
        self.plugins.iter().position(|plugin| plugin.id() == id)
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
            Kind::List
                if self
                    .source(plugin)
                    .is_some_and(|source| source.pending.is_some()) =>
            {
                Vec::new()
            }
            _ => self.ask(plugin),
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
        let Some(target) = self.resolve(item_ref) else {
            self.log.write(format_args!(
                "can't activate {item_ref:?}: no such item; sending the current state"
            ));
            return vec![self.state()];
        };
        let (plugin, item) = match target {
            Target::Hint(plugin) => {
                if action_id != ENTER {
                    self.log.write(format_args!(
                        "can't activate {item_ref:?}: a mode hint has no action {action_id:?}"
                    ));
                    return vec![self.state()];
                }
                return self.enter_mode(plugin);
            }
            Target::Item(plugin, item) => (plugin, item),
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
            plugin: self.plugins[plugin].clone(),
            run: action.run.clone(),
        };
        self.running = Some(Running {
            title: action.title.clone(),
            after: action.after,
        });
        vec![effect]
    }

    /// What a frontend `ref` names, if it still resolves. A plugin's item
    /// or a hint resolves only while the current state shows it: an item
    /// the session holds but doesn't show (the newest text filtered it out)
    /// no longer does. A `[query_item]` resolves while its mode is active,
    /// filled in with the text its ref recorded, even if newer input has
    /// changed the query since (*Items*). That includes while the dedupe
    /// rule hides it: the ref still names what the user saw, and running
    /// it does what the listed item with that label would.
    fn resolve(&self, item_ref: &str) -> Option<Target> {
        let wanted = Ref::parse(item_ref)?;
        if let Ref::QueryItem { plugin, text } = wanted {
            let active = self.mode.as_ref()?;
            let found = &self.plugins[active.plugin];
            if found.id() != plugin || text.is_empty() {
                return None;
            }
            let item = filled_query_item(found.manifest.query_item.as_ref()?, text);
            return Some(Target::Item(active.plugin, item));
        }
        self.visible(&self.shown())
            .into_iter()
            .find_map(|row| match (row, wanted) {
                (Row::Item(source, index), Ref::Item { plugin, item }) => {
                    let source = &self.sources[source];
                    let found = &source.items[index];
                    (self.plugins[source.plugin].id() == plugin && found.id == item)
                        .then(|| Target::Item(source.plugin, found.clone()))
                }
                (Row::Hint(found), Ref::Hint { plugin }) => {
                    (self.plugins[found].id() == plugin).then_some(Target::Hint(found))
                }
                _ => None,
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
    /// their items, as `open` does.
    fn ask_all(&mut self) -> Vec<Effect> {
        let mode = self.mode.as_ref().map(|active| active.plugin);
        let plugins: Vec<usize> = (0..self.plugins.len())
            .filter(|&plugin| is_global(&self.plugins[plugin]) || Some(plugin) == mode)
            .collect();
        plugins
            .into_iter()
            .flat_map(|plugin| self.ask(plugin))
            .collect()
    }

    /// Asks the plugins the state shows for their items again, as a
    /// `refresh` does: the mode's, or in the plain search the global ones.
    fn ask_shown(&mut self) -> Vec<Effect> {
        let plugins: Vec<usize> = match &self.mode {
            Some(active) => vec![active.plugin],
            None => (0..self.plugins.len())
                .filter(|&plugin| is_global(&self.plugins[plugin]))
                .collect(),
        };
        plugins
            .into_iter()
            .flat_map(|plugin| self.ask(plugin))
            .collect()
    }

    /// Asks `plugin` for its items: its list, or its items for the query.
    /// What it gave before stays until it answers.
    fn ask(&mut self, plugin: usize) -> Vec<Effect> {
        let method = match self.plugins[plugin].manifest.kind {
            Kind::List => Method::list(),
            Kind::Query => self.query_method(),
        };
        self.request(plugin, method)
    }

    /// Sends `method` to `plugin`, cancelling its outstanding request. Its
    /// items stay until it answers.
    fn request(&mut self, plugin: usize, method: Method) -> Vec<Effect> {
        let id = self.next_request;
        self.next_request += 1;
        let index = match self.source_index(plugin) {
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

    /// Where `plugin`'s source is in `sources`, or where it would go.
    fn source_index(&self, plugin: usize) -> Result<usize, usize> {
        self.sources
            .binary_search_by_key(&plugin, |source| source.plugin)
    }

    fn source(&self, plugin: usize) -> Option<&Source> {
        self.source_index(plugin)
            .ok()
            .map(|index| &self.sources[index])
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

    /// The rows the current state shows, best first and at most
    /// `max_items`. Items come from the `shown` sources.
    ///
    /// In the plain search: hints whose mode the query names outright, then
    /// the global items, then the other hints (*Filtering and ranking*). In
    /// a list mode, its `[query_item]` comes first.
    fn visible(&self, shown: &[usize]) -> Vec<Row> {
        let query = self.text.trim();
        let mut rows = match &self.mode {
            Some(active) if self.plugins[active.plugin].manifest.kind == Kind::Query => {
                // Shown as given, sorted by score when present.
                let mut rows: Vec<(usize, usize)> = shown
                    .iter()
                    .flat_map(|&source| {
                        (0..self.sources[source].items.len()).map(move |index| (source, index))
                    })
                    .collect();
                let score =
                    |&(source, index): &(usize, usize)| self.sources[source].items[index].score;
                rows.sort_by(|a, b| by_score(score(a), score(b)));
                rows.into_iter()
                    .map(|(source, index)| Row::Item(source, index))
                    .collect()
            }
            Some(active) => {
                let items = self.filtered(shown, query);
                let mut rows = Vec::with_capacity(items.len() + 1);
                if self.shows_query_item(active.plugin, query, &items) {
                    rows.push(Row::QueryItem);
                }
                rows.extend(items);
                rows
            }
            None => {
                let (named, others): (Vec<usize>, Vec<usize>) =
                    self.hinted(query).into_iter().partition(|&plugin| {
                        let plugin = &self.plugins[plugin];
                        matcher::names_mode(query, &plugin.manifest.name, &plugin.prefixes)
                    });
                let mut rows: Vec<Row> = named.into_iter().map(Row::Hint).collect();
                rows.extend(self.filtered(shown, query));
                rows.extend(others.into_iter().map(Row::Hint));
                rows
            }
        };
        rows.truncate(self.max_items);
        rows
    }

    /// The `shown` sources' items that match `query`: by match quality,
    /// then discovery order, then each plugin's order.
    fn filtered(&self, shown: &[usize], query: &str) -> Vec<Row> {
        let mut matches: Vec<(Score, usize, usize)> = shown
            .iter()
            .flat_map(|&source| {
                matcher::filter(&self.sources[source].items, query)
                    .into_iter()
                    .map(move |m| (m.score, source, m.index))
            })
            .collect();
        matches.sort();
        matches
            .into_iter()
            .map(|(_, source, index)| Row::Item(source, index))
            .collect()
    }

    /// The plugins whose mode gets a hint for `query`, in discovery order:
    /// those whose name or one of whose prefixes starts with it, ignoring
    /// case. None for an empty query (*Mode hints*).
    fn hinted(&self, query: &str) -> Vec<usize> {
        (0..self.plugins.len())
            .filter(|&plugin| {
                let plugin = &self.plugins[plugin];
                matcher::starts_with(&plugin.manifest.name, query)
                    || plugin
                        .prefixes
                        .iter()
                        .any(|prefix| matcher::starts_with(prefix, query))
            })
            .collect()
    }

    /// Whether `plugin`'s `[query_item]` is shown above `items`: it has
    /// one, the query isn't empty, and no item still in the filtered list
    /// has exactly its label.
    fn shows_query_item(&self, plugin: usize, query: &str, items: &[Row]) -> bool {
        let Some(query_item) = &self.plugins[plugin].manifest.query_item else {
            return false;
        };
        if query.is_empty() {
            return false;
        }
        let label = fill(&query_item.label, query);
        !items.iter().any(|row| match *row {
            Row::Item(source, index) => self.sources[source].items[index].label == label,
            _ => false,
        })
    }

    /// The state to send now.
    fn state(&self) -> Effect {
        let shown = self.shown();
        let items = self
            .visible(&shown)
            .into_iter()
            .map(|row| self.frontend_row(row))
            .collect();
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

    /// `row` as the frontend draws it.
    fn frontend_row(&self, row: Row) -> FrontendItem {
        match row {
            Row::Item(source, index) => {
                let source = &self.sources[source];
                let plugin = &self.plugins[source.plugin];
                let item = &source.items[index];
                frontend_item(format!("{}/{}", plugin.id(), item.id), plugin, item)
            }
            Row::Hint(plugin) => hint_item(&self.plugins[plugin]),
            Row::QueryItem => {
                let Some(active) = &self.mode else {
                    unreachable!("a [query_item] is only shown in its mode");
                };
                let plugin = &self.plugins[active.plugin];
                let Some(query_item) = &plugin.manifest.query_item else {
                    unreachable!("only a plugin with a [query_item] shows one");
                };
                let query = self.text.trim();
                let item = filled_query_item(query_item, query);
                frontend_item(format!("{}:query:{query}", plugin.id()), plugin, &item)
            }
        }
    }
}

/// The id of a mode hint's one action.
const ENTER: &str = "enter";

/// Whether the plain search shows `plugin`'s items.
fn is_global(plugin: &Plugin) -> bool {
    plugin.manifest.kind == Kind::List && plugin.manifest.global
}

/// The length in bytes of `prefix` at the start of `text`, comparing
/// characters without case.
fn prefix_len(text: &str, prefix: &str) -> Option<usize> {
    // Discovery refuses empty prefixes; this keeps one from matching
    // everything if that ever changes.
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
fn action_key(action: &Action, index: usize) -> String {
    action.id.clone().unwrap_or_else(|| index.to_string())
}

/// `item` as the frontend draws it, with this `ref`: the icon resolved, and
/// nothing about how its actions run.
fn frontend_item(item_ref: String, plugin: &Plugin, item: &Item) -> FrontendItem {
    FrontendItem {
        item_ref,
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

/// The hint for entering `plugin`'s mode (*Mode hints*). Its detail is the
/// prefixes, separated by spaces, and the description, joined by ` · `
/// when it has both, or `None` when it has neither (conflicts can leave a
/// plugin without prefixes).
fn hint_item(plugin: &Plugin) -> FrontendItem {
    let manifest = &plugin.manifest;
    let prefixes = Some(plugin.prefixes.join(" ")).filter(|prefixes| !prefixes.is_empty());
    let detail = match (prefixes, &manifest.description) {
        (Some(prefixes), Some(description)) => Some(format!("{prefixes} · {description}")),
        (prefixes, description) => prefixes.or_else(|| description.clone()),
    };
    FrontendItem {
        item_ref: format!("{}:mode", plugin.id()),
        label: manifest.name.clone(),
        detail,
        icon: manifest.icon.clone(),
        badge: None,
        disabled: false,
        actions: vec![FrontendAction {
            id: ENTER.to_owned(),
            title: format!("Open {}", manifest.name),
            key: None,
        }],
    }
}

/// A `[query_item]` as an item, with `{query}` replaced by `query` in every
/// string. Action ids, keys and a `plugin` run's action id are names, not
/// text, so they stay as written.
fn filled_query_item(query_item: &QueryItem, query: &str) -> Item {
    let fill_all = |strings: &[String]| -> Vec<String> {
        strings.iter().map(|string| fill(string, query)).collect()
    };
    let fill_run = |run: &Run| match run {
        Run::Exec { argv, cwd } => Run::Exec {
            argv: fill_all(argv),
            cwd: cwd.as_deref().map(|cwd| fill(cwd, query)),
        },
        Run::Terminal { argv, cwd } => Run::Terminal {
            argv: fill_all(argv),
            cwd: cwd.as_deref().map(|cwd| fill(cwd, query)),
        },
        Run::App(id) => Run::App(fill(id, query)),
        Run::Open(target) => Run::Open(fill(target, query)),
        Run::Copy(text) => Run::Copy(fill(text, query)),
        Run::Plugin(action) => Run::Plugin(action.clone()),
    };
    Item {
        id: String::new(),
        label: fill(&query_item.label, query),
        detail: query_item
            .detail
            .as_deref()
            .map(|detail| fill(detail, query)),
        icon: query_item.icon.as_ref().map(|icon| match icon {
            Icon::Glyph(glyph) => Icon::Glyph(fill(glyph, query)),
            Icon::Name(name) => Icon::Name(fill(name, query)),
            Icon::Path(path) => Icon::Path(fill(path, query)),
        }),
        badge: query_item.badge.as_deref().map(|badge| fill(badge, query)),
        keywords: Vec::new(),
        disabled: false,
        score: None,
        actions: query_item
            .actions
            .iter()
            .map(|action| Action {
                id: action.id.clone(),
                title: fill(&action.title, query),
                key: action.key.clone(),
                run: fill_run(&action.run),
                after: action.after,
            })
            .collect(),
    }
}

fn fill(text: &str, query: &str) -> String {
    text.replace("{query}", query)
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
