use std::path::PathBuf;
use std::time::Duration;

use quick_launcher_wire::{Action, Icon};

use super::*;
use crate::discovery::{self, Manifest};
use crate::frontend::DEFAULT_MAX_ITEMS;
use crate::log::Lines;

/// A plugin with `extra` added to a minimal manifest.
fn plugin(id: &str, extra: &str) -> Plugin {
    with_prefixes(id, &[&format!("{id}:")], extra)
}

/// A plugin with these prefixes and `extra` added to a minimal manifest.
fn with_prefixes(id: &str, prefixes: &[&str], extra: &str) -> Plugin {
    let prefixes: Vec<String> = prefixes
        .iter()
        .map(|prefix| format!("{prefix:?}"))
        .collect();
    let manifest = Manifest::parse(&format!(
        "protocol = 1\nid = \"{id}\"\nname = \"{}\"\nprefixes = [{}]\nbin = \"bin\"\n{extra}",
        id.to_uppercase(),
        prefixes.join(", ")
    ))
    .unwrap();
    Plugin {
        prefixes: manifest.prefixes.clone(),
        manifest,
        dir: PathBuf::from("/plugins").join(id),
        source: discovery::Source::Bundled,
    }
}

fn global(id: &str) -> Plugin {
    plugin(id, "global = true")
}

fn copy(id: &str, label: &str) -> Item {
    Item {
        id: id.into(),
        label: label.into(),
        detail: None,
        icon: None,
        badge: None,
        keywords: Vec::new(),
        disabled: false,
        score: None,
        actions: vec![Action {
            id: None,
            title: "Copy".into(),
            key: None,
            run: Run::Copy(id.into()),
            after: After::Close,
        }],
    }
}

struct Test {
    session: Session,
    lines: Lines,
}

impl Test {
    fn new(plugins: Vec<Plugin>) -> Test {
        let (log, lines) = Log::memory();
        Test {
            session: Session::new(plugins, log),
            lines,
        }
    }

    fn send(&mut self, message: Incoming) -> Vec<Effect> {
        self.session.handle(Event::Frontend(message))
    }

    /// Opens the session and returns the request id sent to each global
    /// plugin, in order.
    fn open(&mut self) -> Vec<u64> {
        self.send(Incoming::Open { mode: None })
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::Request { request, .. } => Some(request.id),
                _ => None,
            })
            .collect()
    }

    fn answer(&mut self, request: u64, answer: Answer) -> Vec<Effect> {
        self.session.handle(Event::Answer { request, answer })
    }

    fn items(&mut self, request: u64, items: Vec<Item>) -> State {
        only_state(self.answer(request, Answer::Items { items, more: false }))
    }

    fn items_effects(&mut self, request: u64, items: Vec<Item>) -> Vec<Effect> {
        self.answer(request, Answer::Items { items, more: false })
    }

    fn input(&mut self, seq: u64, text: &str) -> State {
        only_state(self.send(Incoming::Input {
            seq,
            mode: None,
            text: text.into(),
        }))
    }

    /// Sends `input` read in `mode`, returning every effect.
    fn input_in(&mut self, seq: u64, mode: Option<&str>, text: &str) -> Vec<Effect> {
        self.send(Incoming::Input {
            seq,
            mode: mode.map(Into::into),
            text: text.into(),
        })
    }

    fn activate(&mut self, item: &str, action: &str) -> Vec<Effect> {
        self.send(Incoming::Activate {
            item: item.into(),
            action: action.into(),
        })
    }
}

fn only_state(effects: Vec<Effect>) -> State {
    match &effects[..] {
        [Effect::Send(Outgoing::State(state))] => state.clone(),
        other => panic!("expected one state, got {other:?}"),
    }
}

fn last_state(effects: &[Effect]) -> State {
    match effects.last() {
        Some(Effect::Send(Outgoing::State(state))) => state.clone(),
        other => panic!("expected a state last, got {other:?}"),
    }
}

fn refs(state: &State) -> Vec<&str> {
    state
        .items
        .iter()
        .map(|item| item.item_ref.as_str())
        .collect()
}

#[test]
fn hello_gets_ready_and_sets_max_items() {
    let mut test = Test::new(vec![global("apps")]);
    let effects = test.send(Incoming::Hello {
        v: 1,
        client: "test".into(),
        client_version: "1".into(),
        max_items: 1,
    });
    assert_eq!(
        effects,
        [Effect::Send(Outgoing::Ready {
            core_version: env!("CARGO_PKG_VERSION").into()
        })]
    );
    let [apps] = test.open()[..] else { panic!() };
    let state = test.items(apps, vec![copy("a", "A"), copy("b", "B")]);
    assert_eq!(refs(&state), ["apps/a"]);
}

#[test]
fn a_version_mismatch_gets_ready_then_exits() {
    let mut test = Test::new(Vec::new());
    let effects = test.send(Incoming::Hello {
        v: 2,
        client: "future".into(),
        client_version: "9".into(),
        max_items: DEFAULT_MAX_ITEMS,
    });
    assert_eq!(
        effects,
        [
            Effect::Send(Outgoing::Ready {
                core_version: env!("CARGO_PKG_VERSION").into()
            }),
            Effect::Exit(1)
        ]
    );
    assert!(test.lines.get()[0].contains("speaks frontend protocol 2"));
}

#[test]
fn open_asks_only_global_list_plugins_and_reports_loading() {
    let mut test = Test::new(vec![
        global("apps"),
        plugin("pi", ""),
        plugin("calc", "kind = \"query\""),
        global("files"),
    ]);
    let effects = test.send(Incoming::Open { mode: None });
    let asked: Vec<(&str, &Method)> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Request { plugin, request } => Some((plugin.id(), &request.method)),
            _ => None,
        })
        .collect();
    assert_eq!(
        asked,
        [("apps", &Method::list()), ("files", &Method::list())]
    );
    let state = last_state(&effects);
    assert!(state.loading);
    assert_eq!(state.items, []);
    assert_eq!(state.mode, None);
}

#[test]
fn each_answer_sends_a_state_and_loading_ends_with_the_last() {
    let mut test = Test::new(vec![global("apps"), global("files")]);
    let [apps, files] = test.open()[..] else {
        panic!()
    };
    let state = test.items(apps, vec![copy("a", "A")]);
    assert!(state.loading);
    assert_eq!(refs(&state), ["apps/a"]);
    let state = test.items(files, vec![copy("f", "F")]);
    assert!(!state.loading);
    assert_eq!(refs(&state), ["apps/a", "files/f"]);
}

#[test]
fn a_partial_answer_keeps_loading() {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    let state = only_state(test.answer(
        apps,
        Answer::Items {
            items: vec![copy("a", "A")],
            more: true,
        },
    ));
    assert!(state.loading);
    assert_eq!(refs(&state), ["apps/a"]);
}

#[test]
fn input_filters_with_the_matcher_and_echoes_seq() {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    test.items(
        apps,
        vec![
            copy("ff-dev", "Firefox Developer Edition"),
            copy("files", "Files"),
            copy("ff", "Firefox"),
            copy("term", "Terminal"),
        ],
    );
    let state = test.input(4, "  fire ");
    assert_eq!(state.seq, 4);
    assert_eq!(state.text, "  fire ", "the text is kept as typed");
    // Both are prefix matches, so they keep the plugin's order.
    assert_eq!(refs(&state), ["apps/ff-dev", "apps/ff"]);
    let state = test.input(5, "firefox");
    assert_eq!(refs(&state), ["apps/ff", "apps/ff-dev"]);
    let state = test.input(6, "");
    assert_eq!(state.items.len(), 4);
}

#[test]
fn ranking_is_match_quality_then_discovery_order_then_plugin_order() {
    let mut test = Test::new(vec![global("apps"), global("files")]);
    let [apps, files] = test.open()[..] else {
        panic!()
    };
    test.items(
        apps,
        vec![copy("code-oss", "Visual Code"), copy("code", "Code")],
    );
    test.items(
        files,
        vec![copy("code-dir", "Code"), copy("codex", "codex notes")],
    );
    let state = test.input(1, "code");
    assert_eq!(
        refs(&state),
        [
            // Exact matches: apps before files.
            "apps/code",
            "files/code-dir",
            // A prefix match.
            "files/codex",
            // A substring match.
            "apps/code-oss",
        ]
    );
    let state = test.input(2, "");
    assert_eq!(
        refs(&state),
        [
            "apps/code-oss",
            "apps/code",
            "files/code-dir",
            "files/codex"
        ]
    );
}

#[test]
fn max_items_caps_every_state() {
    let mut test = Test::new(vec![global("apps")]);
    test.send(Incoming::Hello {
        v: 1,
        client: String::new(),
        client_version: String::new(),
        max_items: 2,
    });
    let [apps] = test.open()[..] else { panic!() };
    let items = (0..5).map(|n| copy(&n.to_string(), "Item")).collect();
    let state = test.items(apps, items);
    assert_eq!(refs(&state), ["apps/0", "apps/1"]);
}

#[test]
fn frontend_items_get_a_ref_an_explicit_icon_and_no_run() {
    let mut test = Test::new(vec![plugin("apps", "global = true\nicon = \"A\"")]);
    let [apps] = test.open()[..] else { panic!() };
    let mut named = copy("x/y.desktop", "Named");
    named.icon = Some(Icon::Name("firefox".into()));
    named.detail = Some("Web Browser".into());
    named.badge = Some("new".into());
    named.actions.push(Action {
        id: Some("open".into()),
        title: "Open".into(),
        key: Some("ctrl+o".into()),
        run: Run::Open("x".into()),
        after: After::Refresh,
    });
    let state = test.items(apps, vec![named, copy("plain", "Plain")]);
    assert_eq!(
        state.items,
        [
            FrontendItem {
                item_ref: "apps/x/y.desktop".into(),
                label: "Named".into(),
                detail: Some("Web Browser".into()),
                icon: Some(Icon::Name("firefox".into())),
                badge: Some("new".into()),
                disabled: false,
                actions: vec![
                    FrontendAction {
                        id: "0".into(),
                        title: "Copy".into(),
                        key: None
                    },
                    FrontendAction {
                        id: "open".into(),
                        title: "Open".into(),
                        key: Some("ctrl+o".into())
                    },
                ],
            },
            FrontendItem {
                item_ref: "apps/plain".into(),
                label: "Plain".into(),
                detail: None,
                icon: Some(Icon::Glyph("A".into())),
                badge: None,
                disabled: false,
                actions: vec![FrontendAction {
                    id: "0".into(),
                    title: "Copy".into(),
                    key: None
                }],
            },
        ]
    );
}

#[test]
fn errors_name_the_plugin_and_are_drawn_alongside_items() {
    let mut test = Test::new(vec![global("apps"), global("files")]);
    let [apps, files] = test.open()[..] else {
        panic!()
    };
    test.items(apps, vec![copy("a", "A")]);
    let state = only_state(test.answer(
        files,
        Answer::Failed(Failure::NoResponse(Duration::from_millis(2000))),
    ));
    assert_eq!(
        state.error.as_deref(),
        Some("FILES didn't respond within 2000 ms")
    );
    assert_eq!(refs(&state), ["apps/a"]);
    assert!(!state.loading);
}

#[test]
fn a_plugin_error_message_is_shown_after_its_name() {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    let state = only_state(test.answer(
        apps,
        Answer::Failed(Failure::Plugin("no desktop entries".into())),
    ));
    assert_eq!(state.error.as_deref(), Some("APPS: no desktop entries"));
}

#[test]
fn a_timeout_after_a_partial_answer_keeps_its_items() {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    test.answer(
        apps,
        Answer::Items {
            items: vec![copy("a", "A")],
            more: true,
        },
    );
    let state = only_state(test.answer(
        apps,
        Answer::Failed(Failure::NoResponse(Duration::from_millis(5))),
    ));
    assert_eq!(refs(&state), ["apps/a"]);
    assert!(state.error.is_some());
}

#[test]
fn reopening_keeps_old_items_until_the_plugin_answers_and_ignores_old_answers() {
    let mut test = Test::new(vec![global("apps")]);
    let [first] = test.open()[..] else { panic!() };
    test.items(first, vec![copy("a", "A")]);
    test.send(Incoming::Close);
    let effects = test.send(Incoming::Open { mode: None });
    let state = last_state(&effects);
    assert!(state.loading);
    assert_eq!(refs(&state), [] as [&str; 0], "close forgets the items");
    let Some(Effect::Request { request, .. }) = effects.first() else {
        panic!()
    };
    let second = request.id;
    assert_ne!(first, second);
    assert_eq!(
        test.answer(
            first,
            Answer::Items {
                items: vec![copy("old", "Old")],
                more: false
            }
        ),
        []
    );
    let state = test.items(second, vec![copy("b", "B")]);
    assert_eq!(refs(&state), ["apps/b"]);
}

#[test]
fn open_resets_the_text_and_keeps_seq() {
    let mut test = Test::new(vec![global("apps")]);
    test.open();
    test.input(3, "fire");
    let state = last_state(&test.send(Incoming::Open { mode: None }));
    assert_eq!(state.text, "");
    assert_eq!(state.seq, 3);
}

#[test]
fn close_and_reopen_cancel_outstanding_requests() {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    assert_eq!(test.send(Incoming::Close), [Effect::Cancel(apps)]);
    let [again] = test.open()[..] else { panic!() };
    let effects = test.send(Incoming::Open { mode: None });
    assert_eq!(effects[0], Effect::Cancel(again));
}

#[test]
fn activating_runs_the_action_then_sends_done() {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![copy("a", "A"), copy("b", "B")]);
    let effects = test.activate("apps/b", "0");
    let [Effect::Run { plugin, run }] = &effects[..] else {
        panic!("{effects:?}")
    };
    assert_eq!(plugin.id(), "apps");
    assert_eq!(run, &Run::Copy("b".into()));
    assert_eq!(
        test.session.handle(Event::Ran(Ok(()))),
        [Effect::Send(Outgoing::Done)]
    );
    // The session is over; nothing can be activated until the next open.
    let state = only_state(test.activate("apps/b", "0"));
    assert_eq!(state.items, []);
}

#[test]
fn an_action_is_found_by_its_own_id() {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    let mut item = copy("a", "A");
    item.actions[0].id = Some("launch".into());
    test.items(apps, vec![item]);
    assert_eq!(test.activate("apps/a", "0").len(), 1, "state, not a run");
    assert!(matches!(
        &test.activate("apps/a", "launch")[..],
        [Effect::Run { .. }]
    ));
}

#[test]
fn an_unknown_ref_or_action_runs_nothing_and_resends_the_state() {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![copy("a", "A")]);
    for (item, action) in [
        ("apps/gone", "0"),
        ("other/a", "0"),
        ("no-slash", "0"),
        ("apps/a", "7"),
    ] {
        let state = only_state(test.activate(item, action));
        assert_eq!(refs(&state), ["apps/a"]);
    }
}

#[test]
fn a_disabled_items_first_action_cannot_run_but_others_can() {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    let mut item = copy("a", "A");
    item.disabled = true;
    item.actions.push(Action {
        id: None,
        title: "Open".into(),
        key: None,
        run: Run::Open("a".into()),
        after: After::Close,
    });
    test.items(apps, vec![item]);
    only_state(test.activate("apps/a", "0"));
    assert!(matches!(
        &test.activate("apps/a", "1")[..],
        [Effect::Run {
            run: Run::Open(_),
            ..
        }]
    ));
}

#[test]
fn a_failed_action_shows_an_error_until_the_next_input() {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![copy("a", "A")]);
    test.activate("apps/a", "0");
    let state = only_state(test.session.handle(Event::Ran(Err("no wl-copy".into()))));
    assert_eq!(
        state.error.as_deref(),
        Some("Couldn't run “Copy” for “A”: no wl-copy")
    );
    assert_eq!(refs(&state), ["apps/a"]);
    assert_eq!(test.input(1, "a").error, None);
}

#[test]
fn a_refresh_action_asks_again_instead_of_closing() {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    let mut item = copy("a", "A");
    item.actions[0].after = After::Refresh;
    test.items(apps, vec![item]);
    test.activate("apps/a", "0");
    let effects = test.session.handle(Event::Ran(Ok(())));
    assert!(matches!(
        &effects[..],
        [Effect::Request { .. }, Effect::Send(Outgoing::State(_))]
    ));
    let state = last_state(&effects);
    assert!(state.loading);
    assert_eq!(refs(&state), ["apps/a"], "old items stay until the answer");
}

#[test]
fn leave_mode_and_complete_do_nothing_without_a_mode_or_a_hint() {
    let mut test = Test::new(vec![global("apps")]);
    test.open();
    assert_eq!(test.send(Incoming::LeaveMode), []);
    assert_eq!(test.send(Incoming::Complete { item: None }), []);
}

#[test]
fn rescanned_plugins_end_the_session_and_apply_from_the_next_open() {
    let mut test = Test::new(vec![global("apps"), global("files")]);
    let [apps, files] = test.open()[..] else {
        panic!()
    };
    test.items(apps, vec![copy("a", "A")]);
    // `files` is removed while its request is outstanding.
    assert_eq!(
        test.session.handle(Event::Plugins(vec![global("apps")])),
        [Effect::Cancel(files)]
    );
    // Its late answer and an activation of an old item touch nothing.
    assert_eq!(test.items_effects(files, vec![copy("f", "F")]), []);
    assert_eq!(
        refs(&only_state(test.activate("apps/a", "0"))),
        [] as [&str; 0]
    );
    let [apps] = test.open()[..] else { panic!() };
    assert_eq!(refs(&test.items(apps, vec![copy("b", "B")])), ["apps/b"]);

    test.session
        .handle(Event::Plugins(vec![global("apps"), global("files")]));
    assert_eq!(test.open().len(), 2);
}

#[test]
fn an_invalid_hello_gets_ready_then_exits() {
    let mut test = Test::new(Vec::new());
    let effects = test.send(Incoming::InvalidHello {
        reason: "missing field `v`".into(),
    });
    assert!(matches!(
        &effects[..],
        [Effect::Send(Outgoing::Ready { .. }), Effect::Exit(1)]
    ));
}

#[test]
fn a_refresh_clears_the_old_error_while_it_asks_again() {
    let mut test = Test::new(vec![global("apps"), global("files")]);
    let [apps, files] = test.open()[..] else {
        panic!()
    };
    let mut item = copy("a", "A");
    item.actions[0].after = After::Refresh;
    test.items(apps, vec![item]);
    test.answer(files, Answer::Failed(Failure::Plugin("broken".into())));
    test.activate("apps/a", "0");
    let state = last_state(&test.session.handle(Event::Ran(Ok(()))));
    assert!(state.loading);
    assert_eq!(state.error, None);
}

#[test]
fn a_refresh_after_a_failed_action_clears_its_error() {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    let mut item = copy("a", "A");
    item.actions[0].after = After::Refresh;
    test.items(apps, vec![item]);
    test.activate("apps/a", "0");
    let state = only_state(test.session.handle(Event::Ran(Err("no wl-copy".into()))));
    assert!(state.error.is_some());
    test.activate("apps/a", "0");
    let state = last_state(&test.session.handle(Event::Ran(Ok(()))));
    assert_eq!(state.error, None);
}

// Modes (M9).

/// The (plugin, method) of every request in `effects`.
fn requests(effects: &[Effect]) -> Vec<(&str, &Method)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Request { plugin, request } => Some((plugin.id(), &request.method)),
            _ => None,
        })
        .collect()
}

fn request_id(effects: &[Effect], plugin_id: &str) -> u64 {
    effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Request { plugin, request } if plugin.id() == plugin_id => Some(request.id),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no request to {plugin_id} in {effects:?}"))
}

fn query(text: &str) -> Method {
    Method::Query(QueryParams { text: text.into() })
}

fn mode_id(state: &State) -> Option<&str> {
    state.mode.as_ref().map(|mode| mode.id.as_str())
}

fn scored(id: &str, score: Option<f64>) -> Item {
    let mut item = copy(id, id);
    item.score = score;
    item
}

/// `apps` (global), `pi` (a list mode) and `calc` (a query mode).
fn modes() -> Test {
    Test::new(vec![
        global("apps"),
        plugin(
            "pi",
            "icon = \"π\"\nplaceholder = \"Open pi in…\"\ndescription = \"Projects\"",
        ),
        plugin("calc", "kind = \"query\""),
    ])
}

#[test]
fn a_prefix_enters_its_mode_and_asks_for_its_list() {
    let mut test = modes();
    test.open();
    let effects = test.input_in(1, None, "pi:");
    assert_eq!(requests(&effects), [("pi", &Method::list())]);
    let state = last_state(&effects);
    assert_eq!(
        state.mode,
        Some(Mode {
            id: "pi".into(),
            name: "PI".into(),
            icon: Some(Icon::Glyph("π".into())),
            placeholder: Some("Open pi in…".into()),
        })
    );
    assert_eq!(state.text, "");
    assert!(state.loading);
    assert_eq!(state.items, []);
    let pi = request_id(&effects, "pi");
    let state = test.items(
        pi,
        vec![copy("ql", "quick-launcher"), copy("dots", "dotfiles")],
    );
    assert_eq!(refs(&state), ["pi/ql", "pi/dots"]);
    assert!(!state.loading);
}

#[test]
fn pi_colon_then_pi_colon_l_both_with_mode_null_enter_pi_once() {
    // The spec's example: both are typed before any reply, so the frontend
    // sends both with `mode: null`.
    let mut test = modes();
    test.open();
    let first = test.input_in(1, None, "pi:");
    let second = test.input_in(2, None, "pi:l");
    assert_eq!(mode_id(&last_state(&first)), Some("pi"));
    assert_eq!(requests(&second), [], "pi is already being asked");
    let state = last_state(&second);
    assert_eq!(mode_id(&state), Some("pi"));
    assert_eq!(state.text, "l");
    assert_eq!(state.seq, 2);
}

#[test]
fn prefixes_match_without_case_and_drop_whitespace_after_them() {
    let mut test = modes();
    test.open();
    let state = last_state(&test.input_in(1, None, "PI:  launch "));
    assert_eq!(mode_id(&state), Some("pi"));
    assert_eq!(state.text, "launch ", "trailing whitespace is kept");
}

#[test]
fn the_longest_matching_prefix_wins() {
    let mut test = Test::new(vec![
        with_prefixes("term", &[">"], ""),
        with_prefixes("root", &[">>"], ""),
    ]);
    test.open();
    let state = last_state(&test.input_in(1, None, ">>ls"));
    assert_eq!(mode_id(&state), Some("root"));
    assert_eq!(state.text, "ls");
    let state = last_state(&test.input_in(2, None, "> ls"));
    assert_eq!(mode_id(&state), Some("term"));
    assert_eq!(state.text, "ls");
}

#[test]
fn text_without_a_prefix_is_the_plain_search() {
    let mut test = modes();
    test.open();
    let state = last_state(&test.input_in(1, None, "pi"));
    assert_eq!(state.mode, None);
    assert_eq!(state.text, "pi");
    let state = last_state(&test.input_in(2, None, " pi:"));
    assert_eq!(state.mode, None, "a prefix only counts at the start");
}

#[test]
fn another_modes_prefix_inside_a_mode_is_search_text() {
    let mut test = modes();
    test.open();
    test.input_in(1, None, "pi:");
    let effects = test.input_in(2, Some("pi"), "calc:2");
    assert_eq!(requests(&effects), []);
    let state = last_state(&effects);
    assert_eq!(mode_id(&state), Some("pi"));
    assert_eq!(state.text, "calc:2");
}

#[test]
fn input_switches_to_the_frontends_mode_first() {
    // The frontend shows pi (say, from a state the core has since moved on
    // from) and the core is in the plain search: read the text in pi.
    let mut test = modes();
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![copy("a", "Alpha")]);
    let effects = test.input_in(1, Some("pi"), "alp");
    assert_eq!(requests(&effects), [("pi", &Method::list())]);
    let state = last_state(&effects);
    assert_eq!(mode_id(&state), Some("pi"));
    assert_eq!(state.text, "alp");
    assert_eq!(refs(&state), [] as [&str; 0], "apps' items aren't pi's");

    // And back: the frontend shows the plain search again.
    let state = last_state(&test.input_in(2, None, "alp"));
    assert_eq!(state.mode, None);
    assert_eq!(refs(&state), ["apps/a"]);
}

#[test]
fn an_unknown_mode_is_read_as_the_plain_search() {
    let mut test = modes();
    test.open();
    let state = last_state(&test.input_in(1, Some("gone"), "pi:x"));
    assert_eq!(mode_id(&state), Some("pi"));
    assert_eq!(state.text, "x");
    assert!(
        test.lines
            .get()
            .iter()
            .any(|line| line.contains("\"gone\""))
    );
}

#[test]
fn a_list_mode_filters_only_its_own_items() {
    let mut test = modes();
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![copy("ql-app", "quick launcher app")]);
    let pi = request_id(&test.input_in(1, None, "pi:"), "pi");
    test.items(
        pi,
        vec![copy("dots", "dotfiles"), copy("ql", "quick-launcher")],
    );
    let state = last_state(&test.input_in(2, Some("pi"), "quick"));
    assert_eq!(refs(&state), ["pi/ql"]);
}

#[test]
fn the_plain_search_keeps_global_items_while_in_a_mode() {
    let mut test = modes();
    let [apps] = test.open()[..] else { panic!() };
    test.input_in(1, None, "pi:");
    // apps answers while pi is shown: its items wait for the plain search.
    let state = test.items(apps, vec![copy("a", "Pine")]);
    assert_eq!(mode_id(&state), Some("pi"));
    assert_eq!(refs(&state), [] as [&str; 0]);
    assert!(state.loading, "pi hasn't answered");
    let state = last_state(&test.send(Incoming::LeaveMode));
    assert_eq!(refs(&state), ["pi:mode", "apps/a"]);
}

#[test]
fn errors_and_loading_are_the_shown_plugins() {
    let mut test = modes();
    let [apps] = test.open()[..] else { panic!() };
    test.answer(apps, Answer::Failed(Failure::Plugin("broken".into())));
    let effects = test.input_in(1, None, "pi:");
    let state = last_state(&effects);
    assert_eq!(state.error, None, "apps' error isn't pi's");
    let pi = request_id(&effects, "pi");
    let state = only_state(test.answer(pi, Answer::Failed(Failure::Plugin("no projects".into()))));
    assert_eq!(state.error.as_deref(), Some("PI: no projects"));
    let state = last_state(&test.send(Incoming::LeaveMode));
    assert_eq!(state.error.as_deref(), Some("APPS: broken"));
}

#[test]
fn a_query_mode_asks_on_every_input_and_cancels_the_last_request() {
    let mut test = modes();
    test.open();
    let effects = test.input_in(1, None, "calc: 2*");
    assert_eq!(requests(&effects), [("calc", &query("2*"))]);
    let first = request_id(&effects, "calc");
    assert!(last_state(&effects).loading);

    let effects = test.input_in(2, Some("calc"), "2*21 ");
    let second = request_id(&effects, "calc");
    assert_eq!(effects[0], Effect::Cancel(first));
    assert_eq!(requests(&effects), [("calc", &query("2*21"))]);

    // The same query (only whitespace changed) isn't asked again.
    let effects = test.input_in(3, Some("calc"), " 2*21");
    assert_eq!(requests(&effects), []);
    assert_eq!(last_state(&effects).text, " 2*21");

    // A superseded answer is ignored; the newest one is shown.
    assert_eq!(test.items_effects(first, vec![scored("old", None)]), []);
    let state = test.items(second, vec![scored("42", None)]);
    assert_eq!(refs(&state), ["calc/42"]);
}

#[test]
fn query_items_are_shown_as_given_sorted_by_score() {
    let mut test = modes();
    test.open();
    let calc = request_id(&test.input_in(1, None, "calc:x"), "calc");
    let state = test.items(
        calc,
        vec![
            scored("none-1", None),
            scored("low", Some(0.1)),
            scored("high", Some(0.9)),
            scored("none-2", None),
            scored("mid", Some(0.5)),
        ],
    );
    assert_eq!(
        refs(&state),
        [
            "calc/high",
            "calc/mid",
            "calc/low",
            "calc/none-1",
            "calc/none-2"
        ],
        "the text doesn't filter them"
    );
}

#[test]
fn leaving_a_query_mode_cancels_its_request() {
    let mut test = modes();
    test.open();
    let calc = request_id(&test.input_in(1, None, "calc:1"), "calc");
    let effects = test.send(Incoming::LeaveMode);
    assert_eq!(effects[0], Effect::Cancel(calc));
    let state = last_state(&effects);
    assert_eq!(state.mode, None);
    assert_eq!(state.text, "calc1");
}

#[test]
fn leave_mode_after_entering_by_prefix_puts_back_that_prefix_less_its_last_character() {
    let mut test = Test::new(vec![
        global("apps"),
        with_prefixes("term", &[">", "term:"], ""),
    ]);
    test.open();
    test.input_in(1, None, "TERM:");
    let state = last_state(&test.send(Incoming::LeaveMode));
    assert_eq!(state.mode, None);
    assert_eq!(state.text, "TERM", "as the user typed it");

    test.input_in(2, None, ">");
    let state = last_state(&test.send(Incoming::LeaveMode));
    assert_eq!(state.text, "");
}

#[test]
fn leave_mode_after_open_puts_back_the_first_prefix_less_its_last_character() {
    let mut test = modes();
    let effects = test.send(Incoming::Open {
        mode: Some("pi".into()),
    });
    let state = last_state(&effects);
    assert_eq!(mode_id(&state), Some("pi"));
    let state = last_state(&test.send(Incoming::LeaveMode));
    assert_eq!(state.mode, None);
    assert_eq!(state.text, "pi");
}

#[test]
fn leave_mode_after_the_frontend_switched_mode_uses_the_first_prefix() {
    let mut test = modes();
    test.open();
    test.input_in(1, None, "PI:");
    // The frontend then reads pi itself: the user still entered it by typing.
    test.input_in(2, Some("pi"), "x");
    let state = last_state(&test.send(Incoming::LeaveMode));
    assert_eq!(state.text, "PIx");

    test.input_in(3, Some("pi"), "");
    let state = last_state(&test.send(Incoming::LeaveMode));
    assert_eq!(
        state.text, "pi",
        "entered by the frontend's mode, not a prefix"
    );
}

#[test]
fn the_state_answering_leave_mode_carries_the_last_seq() {
    let mut test = modes();
    test.open();
    test.input_in(7, None, "pi:");
    let state = last_state(&test.send(Incoming::LeaveMode));
    assert_eq!(state.seq, 7);
}

#[test]
fn leave_mode_without_a_mode_does_nothing() {
    let mut test = modes();
    test.open();
    test.input_in(1, None, "pi");
    assert_eq!(test.send(Incoming::LeaveMode), []);
}

#[test]
fn open_with_a_mode_asks_the_global_plugins_and_the_modes() {
    let mut test = modes();
    let effects = test.send(Incoming::Open {
        mode: Some("pi".into()),
    });
    assert_eq!(
        requests(&effects),
        [("apps", &Method::list()), ("pi", &Method::list())]
    );
    let state = last_state(&effects);
    assert_eq!(mode_id(&state), Some("pi"));
    assert_eq!(state.text, "");

    let effects = test.send(Incoming::Open {
        mode: Some("calc".into()),
    });
    assert_eq!(
        requests(&effects),
        [("apps", &Method::list()), ("calc", &query(""))]
    );
}

#[test]
fn open_without_a_mode_leaves_the_mode() {
    let mut test = modes();
    test.open();
    test.input_in(1, None, "pi:x");
    let state = last_state(&test.send(Incoming::Open { mode: None }));
    assert_eq!(state.mode, None);
    assert_eq!(state.text, "");
}

#[test]
fn open_with_an_unknown_mode_is_the_plain_search() {
    let mut test = modes();
    let state = last_state(&test.send(Incoming::Open {
        mode: Some("gone".into()),
    }));
    assert_eq!(state.mode, None);
    assert!(
        test.lines
            .get()
            .iter()
            .any(|line| line.contains("\"gone\""))
    );
}

#[test]
fn a_global_plugin_is_also_a_mode() {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    // Entering while its request is out doesn't ask again.
    let effects = test.input_in(1, None, "apps:");
    assert_eq!(requests(&effects), []);
    test.items(apps, vec![copy("a", "Alpha")]);
    let state = last_state(&test.input_in(2, Some("apps"), "al"));
    assert_eq!(refs(&state), ["apps/a"]);
    // Leaving doesn't cancel anything the plain search needs.
    let state = last_state(&test.send(Incoming::LeaveMode));
    assert_eq!(state.text, "appsal");
}

#[test]
fn reentering_a_list_mode_asks_again_and_keeps_its_items_meanwhile() {
    let mut test = modes();
    test.open();
    let pi = request_id(&test.input_in(1, None, "pi:"), "pi");
    test.items(pi, vec![copy("ql", "quick-launcher")]);
    test.send(Incoming::LeaveMode);
    let effects = test.input_in(2, None, "pi:");
    assert_eq!(requests(&effects), [("pi", &Method::list())]);
    let state = last_state(&effects);
    assert!(state.loading);
    assert_eq!(refs(&state), ["pi/ql"]);
}

#[test]
fn a_refresh_in_a_mode_asks_only_the_mode_again() {
    let mut test = modes();
    test.open();
    let calc = request_id(&test.input_in(1, None, "calc:1+1"), "calc");
    let mut item = copy("2", "2");
    item.actions[0].after = After::Refresh;
    test.items(calc, vec![item]);
    test.activate("calc/2", "0");
    let effects = test.session.handle(Event::Ran(Ok(())));
    assert_eq!(requests(&effects), [("calc", &query("1+1"))]);
}

#[test]
fn activating_works_in_a_mode() {
    let mut test = modes();
    test.open();
    let pi = request_id(&test.input_in(1, None, "pi:"), "pi");
    test.items(pi, vec![copy("ql", "quick-launcher")]);
    assert!(matches!(
        &test.activate("pi/ql", "0")[..],
        [Effect::Run { .. }]
    ));
}

#[test]
fn an_item_the_newest_text_filtered_out_cannot_be_activated() {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![copy("a", "Alpha"), copy("b", "Beta")]);
    let state = test.input(1, "alp");
    assert_eq!(refs(&state), ["apps/a"]);
    let state = only_state(test.activate("apps/b", "0"));
    assert_eq!(refs(&state), ["apps/a"]);
}

#[test]
fn an_item_past_max_items_cannot_be_activated() {
    let mut test = Test::new(vec![global("apps")]);
    test.send(Incoming::Hello {
        v: 1,
        client: String::new(),
        client_version: String::new(),
        max_items: 1,
    });
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![copy("a", "A"), copy("b", "B")]);
    let state = only_state(test.activate("apps/b", "0"));
    assert_eq!(refs(&state), ["apps/a"]);
    assert!(
        test.lines
            .get()
            .iter()
            .any(|line| line.contains("can't activate \"apps/b\""))
    );
}

#[test]
fn max_items_zero_uses_the_default() {
    let mut test = Test::new(vec![global("apps")]);
    test.send(Incoming::Hello {
        v: 1,
        client: String::new(),
        client_version: String::new(),
        max_items: 0,
    });
    let [apps] = test.open()[..] else { panic!() };
    let items = (0..3).map(|n| copy(&n.to_string(), "Item")).collect();
    assert_eq!(test.items(apps, items).items.len(), 3);
    assert!(
        test.lines
            .get()
            .iter()
            .any(|line| line.contains("max_items 0"))
    );
}

#[test]
fn a_rescan_leaves_the_mode() {
    let mut test = modes();
    test.open();
    test.input_in(1, None, "calc:1");
    test.session.handle(Event::Plugins(vec![global("apps")]));
    // Nothing indexes into the old plugins, even before the next open.
    let state = last_state(&test.input_in(2, None, "x"));
    assert_eq!(state.mode, None);
}

#[test]
fn switching_from_one_mode_to_another_cancels_the_old_ones_request() {
    let mut test = modes();
    test.open();
    let pi = request_id(&test.input_in(1, None, "pi:"), "pi");
    // The frontend shows calc now (say, the user left pi and typed `calc:`).
    let effects = test.input_in(2, Some("calc"), "1");
    assert_eq!(effects[0], Effect::Cancel(pi));
    assert_eq!(requests(&effects), [("calc", &query("1"))]);
    assert_eq!(mode_id(&last_state(&effects)), Some("calc"));
    assert_eq!(test.items_effects(pi, vec![copy("ql", "ql")]), []);
}

#[test]
fn prefixes_and_leave_mode_work_on_characters_not_bytes() {
    let mut test = Test::new(vec![with_prefixes("pi", &["π:"], "")]);
    test.open();
    let state = last_state(&test.input_in(1, None, "π"));
    assert_eq!(state.mode, None, "text shorter than the prefix");
    let state = last_state(&test.input_in(2, None, "π:ab"));
    assert_eq!(mode_id(&state), Some("pi"));
    assert_eq!(state.text, "ab");
    test.input_in(3, Some("pi"), "");
    let state = last_state(&test.send(Incoming::LeaveMode));
    assert_eq!(state.text, "π");
}

#[test]
fn reentering_a_query_mode_asks_again_even_for_the_same_query() {
    let mut test = modes();
    test.open();
    test.input_in(1, None, "calc:1");
    test.send(Incoming::LeaveMode);
    let effects = test.input_in(2, None, "calc:1");
    assert_eq!(requests(&effects), [("calc", &query("1"))]);
}

/// `apps` (global), `pi`, `pics` (whose name starts like pi's) and `term`
/// with two prefixes, all list modes.
fn hinted() -> Test {
    Test::new(vec![
        global("apps"),
        plugin("pi", "icon = \"π\"\ndescription = \"Open pi in a project\""),
        plugin("pics", ""),
        with_prefixes("term", &[">", "term:"], ""),
    ])
}

fn complete(test: &mut Test, item: Option<&str>) -> Vec<Effect> {
    test.send(Incoming::Complete {
        item: item.map(Into::into),
    })
}

#[test]
fn a_hint_is_an_item_built_from_the_mode() {
    let mut test = hinted();
    test.open();
    let state = test.input(1, "pi");
    let hint = &state.items[0];
    assert_eq!(
        hint,
        &FrontendItem {
            item_ref: "pi:mode".into(),
            label: "PI".into(),
            detail: Some("pi: · Open pi in a project".into()),
            icon: Some(Icon::Glyph("π".into())),
            badge: None,
            disabled: false,
            actions: vec![FrontendAction {
                id: "enter".into(),
                title: "Open PI".into(),
                key: None,
            }],
        }
    );
    // Without a description the detail is the prefixes alone.
    let state = test.input(2, "ter");
    assert_eq!(refs(&state), ["term:mode"]);
    assert_eq!(state.items[0].detail.as_deref(), Some("> term:"));
}

#[test]
fn hints_match_the_start_of_a_name_or_a_prefix_ignoring_case() {
    let mut test = hinted();
    test.open();
    assert_eq!(refs(&test.input(1, "P")), ["pi:mode", "pics:mode"]);
    assert_eq!(refs(&test.input(2, "TER")), ["term:mode"]);
    // `>` is a prefix, so typing it enters the mode instead.
    assert_eq!(refs(&test.input(3, "erm")), [] as [&str; 0]);
    assert_eq!(
        refs(&test.input(4, "  ")),
        [] as [&str; 0],
        "no hints when empty"
    );
}

#[test]
fn the_plain_search_is_named_modes_then_global_items_then_other_modes() {
    // v0's order: the mode the query names, the apps, the other modes.
    let mut test = hinted();
    let [apps] = test.open()[..] else { panic!() };
    test.items(
        apps,
        vec![
            copy("spider", "Spider"),
            copy("pinta", "Pinta"),
            copy("gimp", "GIMP"),
        ],
    );
    let state = test.input(1, "pi");
    assert_eq!(
        refs(&state),
        ["pi:mode", "apps/pinta", "apps/spider", "pics:mode"]
    );
    // `term` names the mode by its prefix without the `:`.
    assert_eq!(refs(&test.input(2, "term"))[0], "term:mode");
    // A query no mode is named by puts every hint after the items.
    assert_eq!(refs(&test.input(3, "pic")), ["pics:mode"], "no app matches");
    assert_eq!(
        refs(&test.input(4, "p")),
        [
            "apps/pinta",
            "apps/spider",
            "apps/gimp",
            "pi:mode",
            "pics:mode"
        ]
    );
}

#[test]
fn several_named_hints_keep_discovery_order() {
    let mut test = Test::new(vec![
        global("apps"),
        with_prefixes("one", &["o:"], ""),
        with_prefixes("two", &["one:"], ""),
    ]);
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![copy("onenote", "OneNote")]);
    // `one` is the first's name and the second's prefix less its `:`.
    assert_eq!(
        refs(&test.input(1, "one")),
        ["one:mode", "two:mode", "apps/onenote"]
    );
}

#[test]
fn hints_count_toward_max_items() {
    let mut test = hinted();
    test.send(Incoming::Hello {
        v: 1,
        client: "test".into(),
        client_version: "1".into(),
        max_items: 2,
    });
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![copy("pinta", "Pinta"), copy("spider", "Spider")]);
    assert_eq!(refs(&test.input(1, "pi")), ["pi:mode", "apps/pinta"]);
    // A hint cut off by max_items can't be activated.
    let state = only_state(test.activate("pics:mode", "enter"));
    assert_eq!(mode_id(&state), None);
}

#[test]
fn hints_are_only_in_the_plain_search() {
    let mut test = hinted();
    test.open();
    let state = last_state(&test.input_in(1, Some("pics"), "pi"));
    assert_eq!(mode_id(&state), Some("pics"));
    assert_eq!(refs(&state), [] as [&str; 0]);
}

#[test]
fn complete_enters_the_first_hints_mode_with_empty_text() {
    let mut test = hinted();
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![copy("pinta", "Pinta")]);
    test.input(5, "pic");
    let effects = complete(&mut test, None);
    assert_eq!(requests(&effects), [("pics", &Method::list())]);
    let state = last_state(&effects);
    assert_eq!(mode_id(&state), Some("pics"));
    assert_eq!(state.text, "");
    assert_eq!(state.seq, 5, "carries the last seq");
    // Entered without a prefix, so leaving puts back its first one.
    let state = last_state(&test.send(Incoming::LeaveMode));
    assert_eq!(state.text, "pics");
}

#[test]
fn complete_without_a_selected_hint_skips_the_items() {
    let mut test = hinted();
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![copy("pinta", "Pinta")]);
    test.input(1, "p");
    // The selected row is an app, so the first hint is completed.
    let state = last_state(&complete(&mut test, Some("apps/pinta")));
    assert_eq!(mode_id(&state), Some("pi"));
}

#[test]
fn complete_enters_the_selected_hints_mode() {
    let mut test = hinted();
    test.open();
    test.input(1, "p");
    let state = last_state(&complete(&mut test, Some("pics:mode")));
    assert_eq!(mode_id(&state), Some("pics"));
}

#[test]
fn complete_with_a_hint_seen_before_a_prefix_entered_a_mode() {
    // The user typed `pi:` and pressed Tab on `term`'s hint, which they
    // still saw, before the core's state for `pi:` arrived.
    let mut test = hinted();
    test.open();
    test.input(1, "p");
    let pi = request_id(&test.input_in(2, None, "pi:"), "pi");
    let effects = complete(&mut test, Some("term:mode"));
    assert_eq!(effects[0], Effect::Cancel(pi));
    assert_eq!(requests(&effects), [("term", &Method::list())]);
    assert_eq!(mode_id(&last_state(&effects)), Some("term"));
}

#[test]
fn complete_does_nothing_without_a_hint() {
    let mut test = hinted();
    test.open();
    test.input(1, "zzz");
    assert_eq!(complete(&mut test, None), []);
    assert_eq!(complete(&mut test, Some("nope:mode")), []);
    // A mode shows no hints, so there is nothing to complete.
    test.input_in(2, None, "pi:");
    assert_eq!(complete(&mut test, None), []);
    assert!(
        test.lines
            .get()
            .iter()
            .any(|line| line.contains("nothing to complete"))
    );
}

#[test]
fn activating_a_hint_enters_its_mode_and_sends_state_not_done() {
    let mut test = hinted();
    test.open();
    test.input(3, "te");
    let effects = test.activate("term:mode", "enter");
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Run { .. } | Effect::Send(Outgoing::Done))),
        "{effects:?}"
    );
    assert_eq!(requests(&effects), [("term", &Method::list())]);
    let state = last_state(&effects);
    assert_eq!(mode_id(&state), Some("term"));
    assert_eq!((state.text.as_str(), state.seq), ("", 3));
    let state = last_state(&test.send(Incoming::LeaveMode));
    assert_eq!(state.text, "", "`>` less its last character");
}

#[test]
fn a_hint_has_only_the_enter_action_and_must_be_shown() {
    let mut test = hinted();
    test.open();
    test.input(1, "pi");
    let state = only_state(test.activate("pi:mode", "0"));
    assert_eq!(mode_id(&state), None);
    // `term`'s hint isn't shown for `pi`.
    let state = only_state(test.activate("term:mode", "enter"));
    assert_eq!(mode_id(&state), None);
}

/// `apps` (global) and `term`, a list mode with a `[query_item]`.
fn with_query_item() -> Test {
    Test::new(vec![
        global("apps"),
        plugin(
            "term",
            r#"icon = ">"
[query_item]
label = "{query}"
detail = "Run {query} in a terminal"
actions = [
  { id = "run", title = "Run {query}", key = "ctrl+r", run = { terminal = ["sh", "-c", "{query}"], cwd = "~/{query}" } },
  { title = "Copy", run = { copy = "{query}" } },
]
"#,
        ),
    ])
}

/// Enters `term` and answers its list with `items`.
fn in_term(test: &mut Test, items: Vec<Item>) {
    test.open();
    let term = request_id(&test.input_in(1, None, "term:"), "term");
    test.items(term, items);
}

#[test]
fn a_query_item_is_filled_in_and_shown_first() {
    let mut test = with_query_item();
    in_term(
        &mut test,
        vec![copy("ls-la", "ls -la"), copy("lsblk", "lsblk")],
    );
    let state = last_state(&test.input_in(2, Some("term"), "ls  "));
    assert_eq!(refs(&state), ["term:query:ls", "term/ls-la", "term/lsblk"]);
    assert_eq!(state.text, "ls  ", "the state keeps trailing whitespace");
    assert_eq!(
        state.items[0],
        FrontendItem {
            item_ref: "term:query:ls".into(),
            label: "ls".into(),
            detail: Some("Run ls in a terminal".into()),
            icon: Some(Icon::Glyph(">".into())),
            badge: None,
            disabled: false,
            actions: vec![
                FrontendAction {
                    id: "run".into(),
                    title: "Run ls".into(),
                    key: Some("ctrl+r".into()),
                },
                FrontendAction {
                    id: "1".into(),
                    title: "Copy".into(),
                    key: None,
                },
            ],
        }
    );
}

#[test]
fn a_query_item_is_hidden_for_an_empty_query() {
    let mut test = with_query_item();
    in_term(&mut test, vec![copy("ls", "ls")]);
    let state = last_state(&test.input_in(2, Some("term"), "   "));
    assert_eq!(refs(&state), ["term/ls"]);
}

#[test]
fn a_query_item_is_hidden_while_a_filtered_item_has_exactly_its_label() {
    let mut test = with_query_item();
    in_term(&mut test, vec![copy("ls-la", "ls -la"), copy("ls", "ls")]);
    let state = last_state(&test.input_in(2, Some("term"), "ls"));
    assert_eq!(refs(&state), ["term/ls", "term/ls-la"]);
    // A label that only differs in case is another command.
    let state = last_state(&test.input_in(3, Some("term"), "LS"));
    assert_eq!(refs(&state), ["term:query:LS", "term/ls", "term/ls-la"]);
}

#[test]
fn a_query_item_is_only_in_its_mode() {
    let mut test = Test::new(vec![plugin(
        "hist",
        "global = true\n[query_item]\nlabel = \"{query}\"\nactions = [{ title = \"Run\", run = { copy = \"{query}\" } }]",
    )]);
    let [hist] = test.open()[..] else { panic!() };
    test.items(hist, vec![copy("ls", "lsblk")]);
    assert_eq!(refs(&test.input(1, "ls")), ["hist/ls"]);
}

#[test]
fn query_item_refs_never_equal_an_items() {
    let mut test = with_query_item();
    // Items whose ids look like the core's own refs.
    in_term(
        &mut test,
        vec![copy(":query:x", "x one"), copy("mode", "x two")],
    );
    let state = last_state(&test.input_in(2, Some("term"), "x"));
    assert_eq!(refs(&state), ["term:query:x", "term/:query:x", "term/mode"]);
    // Each ref still runs its own item.
    let run = |test: &mut Test, item: &str, action: &str| match &test.activate(item, action)[..] {
        [Effect::Run { run, .. }] => run.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(run(&mut test, "term:query:x", "1"), Run::Copy("x".into()));
    test.session.handle(Event::Ran(Err("try again".into())));
    assert_eq!(
        run(&mut test, "term/:query:x", "0"),
        Run::Copy(":query:x".into())
    );
    test.session.handle(Event::Ran(Err("try again".into())));
    assert_eq!(run(&mut test, "term/mode", "0"), Run::Copy("mode".into()));
}

#[test]
fn a_query_item_runs_with_the_text_it_was_built_from() {
    let mut test = with_query_item();
    in_term(&mut test, Vec::new());
    test.input_in(2, Some("term"), "ls");
    // Newer input arrives before the user's Enter on `ls`.
    test.input_in(3, Some("term"), "lsb");
    let [Effect::Run { plugin, run }] = &test.activate("term:query:ls", "run")[..] else {
        panic!()
    };
    assert_eq!(plugin.id(), "term");
    assert_eq!(
        run,
        &Run::Terminal {
            argv: vec!["sh".into(), "-c".into(), "ls".into()],
            cwd: Some("~/ls".into()),
        }
    );
}

#[test]
fn a_query_item_resolves_only_in_its_mode() {
    let mut test = with_query_item();
    in_term(&mut test, Vec::new());
    test.input_in(2, Some("term"), "ls");
    test.send(Incoming::LeaveMode);
    only_state(test.activate("term:query:ls", "run"));
    test.input_in(3, None, "term:");
    // An empty recorded text was never shown.
    only_state(test.activate("term:query:", "run"));
    assert!(matches!(
        &test.activate("term:query:ls", "run")[..],
        [Effect::Run { .. }]
    ));
}

#[test]
fn a_hint_without_prefixes_has_only_its_description_or_no_detail() {
    // Conflicts can take every prefix of a later plugin.
    let mut described = plugin("pi", "description = \"Projects\"");
    described.prefixes.clear();
    let mut bare = plugin("pics", "");
    bare.prefixes.clear();
    let mut test = Test::new(vec![described, bare]);
    test.open();
    let state = test.input(1, "pi");
    assert_eq!(refs(&state), ["pi:mode", "pics:mode"]);
    assert_eq!(state.items[0].detail.as_deref(), Some("Projects"));
    assert_eq!(state.items[1].detail, None);
}

#[test]
fn complete_takes_a_hint_the_state_no_longer_shows_but_activate_does_not() {
    let mut test = hinted();
    test.open();
    test.input(1, "zzz");
    // Enter on the stale hint runs nothing and resends the state.
    let state = only_state(test.activate("pi:mode", "enter"));
    assert_eq!(mode_id(&state), None);
    // Tab on it enters the mode the user saw.
    let state = last_state(&complete(&mut test, Some("pi:mode")));
    assert_eq!(mode_id(&state), Some("pi"));
}

#[test]
fn a_query_item_hidden_by_the_dedupe_still_runs_from_its_ref() {
    let mut test = with_query_item();
    in_term(&mut test, vec![copy("ls", "ls")]);
    let state = last_state(&test.input_in(2, Some("term"), "ls"));
    assert_eq!(refs(&state), ["term/ls"]);
    assert!(matches!(
        &test.activate("term:query:ls", "1")[..],
        [Effect::Run { run: Run::Copy(text), .. }] if text == "ls"
    ));
}

// Activation (M11).

/// An item whose only action is the `plugin` action `action`, with `after`
/// set to `refresh` to show that it's ignored.
fn pinnable(id: &str, label: &str, action: &str) -> Item {
    let mut item = copy(id, label);
    item.actions[0] = Action {
        id: Some(action.into()),
        title: action.to_uppercase(),
        key: None,
        run: Run::Plugin(action.into()),
        after: After::Refresh,
    };
    item
}

/// The `activate` request in `effects`: its id, the plugin it goes to and
/// its params.
fn activate_request(effects: &[Effect]) -> (u64, String, ActivateParams) {
    match effects {
        [
            Effect::Request {
                plugin,
                request:
                    Request {
                        id,
                        method: Method::Activate(params),
                        ..
                    },
            },
        ] => (*id, plugin.id().to_owned(), params.clone()),
        other => panic!("expected one activate request, got {other:?}"),
    }
}

fn result(close: bool, refresh: bool, query: Option<&str>) -> Answer {
    Answer::Result(ActivateResult {
        close,
        refresh,
        query: query.map(Into::into),
        run: None,
    })
}

/// `apps` (global) with `a` and `b` listed, `b` pinnable, and `b` typed.
fn with_pinnable() -> (Test, u64) {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![copy("a", "A"), pinnable("b", "B", "pin")]);
    test.input(1, " b ");
    (test, apps)
}

#[test]
fn a_plugin_action_sends_activate_with_the_item_and_the_query() {
    let (mut test, apps) = with_pinnable();
    let (id, plugin, params) = activate_request(&test.activate("apps/b", "pin"));
    assert_eq!(plugin, "apps");
    assert_ne!(id, apps);
    assert_eq!(
        params,
        ActivateParams {
            item: Some("b".into()),
            action: "pin".into(),
            text: "b".into(),
        }
    );
    // Nothing runs through the platform, so no `Ran` is awaited.
    assert_eq!(test.session.handle(Event::Ran(Ok(()))), []);
}

#[test]
fn a_closing_result_ends_the_session_whatever_after_says() {
    let (mut test, _) = with_pinnable();
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    assert_eq!(
        test.answer(id, result(true, false, None)),
        [Effect::Send(Outgoing::Done)]
    );
    assert_eq!(only_state(test.activate("apps/b", "pin")).items, []);
}

#[test]
fn a_refresh_result_asks_the_shown_plugins_again_and_stays_open() {
    let (mut test, _) = with_pinnable();
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    // `refresh` implies staying open, even with `close` left at its default.
    let effects = test.answer(id, result(true, true, None));
    let [
        Effect::Request { request, .. },
        Effect::Send(Outgoing::State(state)),
    ] = &effects[..]
    else {
        panic!("{effects:?}")
    };
    assert_eq!(request.method, Method::list());
    assert!(state.loading);
    assert_eq!(refs(state), ["apps/b"], "old items stay until the answer");
    let state = test.items(request.id, vec![copy("b", "B pinned")]);
    assert_eq!(state.items[0].label, "B pinned");
}

#[test]
fn a_refresh_result_in_a_mode_asks_only_the_mode() {
    let mut test = Test::new(vec![global("apps"), plugin("pi", "")]);
    test.open();
    let pi = request_id(&test.input_in(1, None, "pi:"), "pi");
    test.items(pi, vec![pinnable("x", "X", "unpin")]);
    let (id, plugin, _) = activate_request(&test.activate("pi/x", "unpin"));
    assert_eq!(plugin, "pi");
    let effects = test.answer(id, result(false, true, None));
    assert_eq!(requests(&effects), [("pi", &Method::list())]);
    assert_eq!(mode_id(&last_state(&effects)), Some("pi"));
}

#[test]
fn a_query_result_replaces_the_text() {
    let (mut test, _) = with_pinnable();
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    let state = only_state(test.answer(id, result(false, false, Some("a"))));
    assert_eq!(state.text, "a");
    assert_eq!(state.seq, 1, "seq stays, as for leave_mode");
    assert_eq!(refs(&state), ["apps/a", "apps:mode"]);
}

#[test]
fn a_query_result_with_a_prefix_in_the_plain_search_enters_its_mode() {
    let mut test = Test::new(vec![global("apps"), plugin("pi", "")]);
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![pinnable("b", "B", "pin")]);
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    let effects = test.answer(id, result(false, false, Some("pi: x")));
    assert_eq!(requests(&effects), [("pi", &Method::list())]);
    let state = last_state(&effects);
    assert_eq!((mode_id(&state), state.text.as_str()), (Some("pi"), "x"));
    // Entered by that prefix, so leave_mode puts it back.
    let state = last_state(&test.send(Incoming::LeaveMode));
    assert_eq!(state.text, "pix");
}

#[test]
fn a_query_result_with_a_prefix_and_refresh_asks_the_new_mode_once() {
    let mut test = Test::new(vec![global("apps"), plugin("pi", "")]);
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![pinnable("b", "B", "pin")]);
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    let effects = test.answer(id, result(false, true, Some("pi:")));
    assert_eq!(requests(&effects), [("pi", &Method::list())]);
    assert_eq!(mode_id(&last_state(&effects)), Some("pi"));
}

#[test]
fn a_query_result_with_a_prefix_in_a_mode_is_text() {
    let mut test = Test::new(vec![plugin("pi", ""), plugin("term", "")]);
    test.open();
    let pi = request_id(&test.input_in(1, None, "pi:"), "pi");
    test.items(pi, vec![pinnable("x", "X", "use")]);
    let (id, ..) = activate_request(&test.activate("pi/x", "use"));
    let effects = test.answer(id, result(false, false, Some("term:ls")));
    assert_eq!(requests(&effects), []);
    let state = last_state(&effects);
    assert_eq!(
        (mode_id(&state), state.text.as_str()),
        (Some("pi"), "term:ls")
    );
}

#[test]
fn a_query_result_is_dropped_once_the_mode_changed_but_refresh_still_applies() {
    let mut test = Test::new(vec![global("apps"), plugin("pi", "")]);
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![copy("a", "A")]);
    let pi = request_id(&test.input_in(1, None, "pi:"), "pi");
    test.items(pi, vec![pinnable("x", "X", "use")]);
    test.input_in(2, Some("pi"), "x");
    let (id, ..) = activate_request(&test.activate("pi/x", "use"));
    let state = last_state(&test.send(Incoming::LeaveMode));
    assert_eq!(state.text, "pix");
    let effects = test.answer(id, result(false, true, Some("other")));
    assert_eq!(requests(&effects), [("apps", &Method::list())]);
    let state = last_state(&effects);
    assert_eq!((mode_id(&state), state.text.as_str()), (None, "pix"));
    assert!(
        test.lines
            .get()
            .iter()
            .any(|line| line.contains("the mode changed"))
    );
}

#[test]
fn newer_input_in_the_same_mode_does_not_drop_a_query_result() {
    let (mut test, _) = with_pinnable();
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    test.input(2, "typed");
    let state = only_state(test.answer(id, result(false, false, Some("a"))));
    assert_eq!((state.seq, state.text.as_str()), (2, "a"));
}

#[test]
fn an_activate_answer_after_a_reopen_or_a_rescan_is_ignored() {
    let (mut test, _) = with_pinnable();
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    let [apps] = test.open()[..] else { panic!() };
    assert_eq!(test.answer(id, result(true, false, None)), []);
    test.items(apps, vec![pinnable("b", "B", "pin")]);
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    // The rescan removes the plugin the activation indexes.
    assert_eq!(test.session.handle(Event::Plugins(Vec::new())), []);
    assert_eq!(test.answer(id, result(false, true, None)), []);
}

#[test]
fn a_query_result_in_a_query_mode_asks_for_the_new_query() {
    let mut test = Test::new(vec![plugin("calc", "kind = \"query\"")]);
    test.open();
    let calc = request_id(&test.input_in(1, None, "calc: 2*"), "calc");
    test.items(calc, vec![pinnable("x", "X", "use")]);
    let (id, ..) = activate_request(&test.activate("calc/x", "use"));
    let effects = test.answer(id, result(false, false, Some("2*21")));
    assert_eq!(requests(&effects), [("calc", &query("2*21"))]);
    assert_eq!(last_state(&effects).text, "2*21");
    // With `refresh` too, it's asked once, for the new query.
    let calc = request_id(&effects, "calc");
    test.items(calc, vec![pinnable("x", "X", "use")]);
    let (id, ..) = activate_request(&test.activate("calc/x", "use"));
    let effects = test.answer(id, result(false, true, Some("42")));
    assert_eq!(requests(&effects), [("calc", &query("42"))]);
}

#[test]
fn a_result_that_neither_closes_nor_refreshes_resends_the_state() {
    let (mut test, _) = with_pinnable();
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    let state = only_state(test.answer(id, result(false, false, None)));
    assert_eq!((state.text.as_str(), refs(&state)), (" b ", vec!["apps/b"]));
}

#[test]
fn an_activate_error_keeps_the_items_and_shows_until_the_next_input() {
    let (mut test, _) = with_pinnable();
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    let failed = Answer::Failed(Failure::Plugin("pins.json is not valid JSON".into()));
    let state = only_state(test.answer(id, failed));
    assert_eq!(
        state.error.as_deref(),
        Some("Couldn't run “PIN” for “B”: pins.json is not valid JSON")
    );
    assert_eq!(refs(&state), ["apps/b"]);
    assert_eq!(test.input(2, "b").error, None);

    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    let timeout = Answer::Failed(Failure::NoResponse(Duration::from_millis(2000)));
    assert_eq!(
        only_state(test.answer(id, timeout)).error.as_deref(),
        Some("Couldn't run “PIN” for “B”: APPS didn't respond within 2000 ms")
    );

    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    let crash = Answer::Failed(Failure::Exited(None));
    assert_eq!(
        only_state(test.answer(id, crash)).error.as_deref(),
        Some("Couldn't run “PIN” for “B”: APPS stopped without answering")
    );
}

fn with_run(answer: Answer, run: Run) -> Answer {
    let Answer::Result(result) = answer else {
        panic!("{answer:?}")
    };
    Answer::Result(ActivateResult {
        run: Some(run),
        ..result
    })
}

#[test]
fn a_run_in_a_result_runs_from_the_plugin_then_the_result_applies() {
    let (mut test, _) = with_pinnable();
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    let terminal = Run::Terminal {
        argv: vec!["ls".into()],
        cwd: None,
    };
    let effects = test.answer(id, with_run(result(true, false, None), terminal.clone()));
    let [Effect::Run { plugin, run }] = &effects[..] else {
        panic!("{effects:?}")
    };
    assert_eq!((plugin.id(), run), ("apps", &terminal));
    assert_eq!(
        test.session.handle(Event::Ran(Ok(()))),
        [Effect::Send(Outgoing::Done)]
    );

    // With `refresh`, the launcher stays open once it ran.
    let mut test = with_pinnable().0;
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    let effects = test.answer(id, with_run(result(false, true, None), terminal));
    assert!(matches!(&effects[..], [Effect::Run { .. }]), "{effects:?}");
    let effects = test.session.handle(Event::Ran(Ok(())));
    assert_eq!(requests(&effects), [("apps", &Method::list())]);
}

#[test]
fn a_run_in_a_result_then_applies_its_query_unless_the_mode_changed() {
    let copy = Run::Copy("b".into());
    let (mut test, _) = with_pinnable();
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    let effects = test.answer(id, with_run(result(false, false, Some("a")), copy.clone()));
    assert!(matches!(&effects[..], [Effect::Run { .. }]), "{effects:?}");
    let state = only_state(test.session.handle(Event::Ran(Ok(()))));
    assert_eq!(state.text, "a");

    // A mode entered while it ran drops the query, as without a run.
    let mut test = Test::new(vec![global("apps"), plugin("pi", "")]);
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![pinnable("b", "B", "pin")]);
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    test.answer(id, with_run(result(false, false, Some("a")), copy));
    test.input_in(1, None, "pi:");
    let state = last_state(&test.session.handle(Event::Ran(Ok(()))));
    assert_eq!((mode_id(&state), state.text.as_str()), (Some("pi"), ""));
}

#[test]
fn a_run_in_a_result_that_fails_keeps_the_launcher_open_with_the_error() {
    let (mut test, _) = with_pinnable();
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    let copy = Run::Copy("b".into());
    test.answer(id, with_run(result(true, false, None), copy));
    let state = only_state(test.session.handle(Event::Ran(Err("no wl-copy".into()))));
    assert_eq!(
        state.error.as_deref(),
        Some("Couldn't run “PIN” for “B”: no wl-copy")
    );
    assert_eq!(refs(&state), ["apps/b"]);
}

#[test]
fn a_plugin_run_in_a_result_runs_nothing() {
    let (mut test, _) = with_pinnable();
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    let effects = test.answer(
        id,
        with_run(result(true, false, None), Run::Plugin("pin".into())),
    );
    let state = only_state(effects);
    assert_eq!(
        state.error.as_deref(),
        Some("Couldn't run “PIN” for “B”: APPS answered with another plugin action")
    );
    assert_eq!(test.session.handle(Event::Ran(Ok(()))), []);
}

#[test]
fn an_activate_answer_after_the_session_ended_is_ignored() {
    let (mut test, _) = with_pinnable();
    let (id, ..) = activate_request(&test.activate("apps/b", "pin"));
    // The activate isn't cancelled: the plugin may be writing its pins.
    assert_eq!(test.send(Incoming::Close), []);
    assert_eq!(test.answer(id, result(true, false, None)), []);
}

#[test]
fn only_the_newest_activation_is_applied() {
    let (mut test, _) = with_pinnable();
    let (first, ..) = activate_request(&test.activate("apps/b", "pin"));
    let (second, ..) = activate_request(&test.activate("apps/b", "pin"));
    assert_eq!(test.answer(first, result(true, false, None)), []);
    assert_eq!(
        test.answer(second, result(true, false, None)),
        [Effect::Send(Outgoing::Done)]
    );
}

#[test]
fn a_disabled_items_first_plugin_action_cannot_run_but_others_can() {
    let mut test = Test::new(vec![global("pi")]);
    let [pi] = test.open()[..] else { panic!() };
    let mut item = pinnable("x", "X", "open");
    item.disabled = true;
    item.actions
        .push(pinnable("x", "X", "unpin").actions.remove(0));
    test.items(pi, vec![item]);
    only_state(test.activate("pi/x", "open"));
    let (_, _, params) = activate_request(&test.activate("pi/x", "unpin"));
    assert_eq!(params.action, "unpin");
}

#[test]
fn an_unknown_ref_sends_no_activate() {
    let (mut test, _) = with_pinnable();
    let state = only_state(test.activate("apps/gone", "pin"));
    assert_eq!(refs(&state), ["apps/b"]);
}

/// `term` as in [`with_query_item`], with a `plugin` action that runs the
/// typed command and records it.
fn with_plugin_query_item() -> Test {
    Test::new(vec![plugin(
        "term",
        r#"[query_item]
label = "{query}"
actions = [{ id = "run", title = "Run {query}", run = { plugin = "run-{query}" } }]
"#,
    )])
}

#[test]
fn a_query_items_plugin_action_sends_a_null_item_and_the_recorded_text() {
    let mut test = with_plugin_query_item();
    in_term(&mut test, Vec::new());
    test.input_in(2, Some("term"), "ls");
    // Newer input arrives before the user's Enter on `ls`.
    test.input_in(3, Some("term"), "lsb");
    let (_, plugin, params) = activate_request(&test.activate("term:query:ls", "run"));
    assert_eq!(plugin, "term");
    assert_eq!(
        params,
        ActivateParams {
            item: None,
            // A `plugin` run's action id is a name, so it isn't filled in.
            action: "run-{query}".into(),
            text: "ls".into(),
        }
    );
}
