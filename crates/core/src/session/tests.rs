use std::path::PathBuf;
use std::time::Duration;

use quick_launcher_wire::{Action, Icon};

use super::*;
use crate::discovery::{self, Manifest};
use crate::frontend::DEFAULT_MAX_ITEMS;
use crate::log::Lines;

/// A plugin with `extra` added to a minimal manifest.
fn plugin(id: &str, extra: &str) -> Plugin {
    let manifest = Manifest::parse(&format!(
        "protocol = 1\nid = \"{id}\"\nname = \"{}\"\nprefixes = [\"{id}:\"]\nbin = \"bin\"\n{extra}",
        id.to_uppercase()
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
fn plugin_actions_are_not_run_yet() {
    let mut test = Test::new(vec![global("apps")]);
    let [apps] = test.open()[..] else { panic!() };
    let mut item = copy("a", "A");
    item.actions[0].run = Run::Plugin("pin".into());
    test.items(apps, vec![item]);
    only_state(test.activate("apps/a", "0"));
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
        Some("Couldn't run “Copy”: no wl-copy")
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
fn leave_mode_and_complete_do_nothing_without_modes() {
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
fn open_with_a_mode_is_the_plain_search_for_now() {
    let mut test = Test::new(vec![global("apps"), plugin("pi", "")]);
    let effects = test.send(Incoming::Open {
        mode: Some("pi".into()),
    });
    let asked: Vec<&str> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Request { plugin, .. } => Some(plugin.id()),
            _ => None,
        })
        .collect();
    assert_eq!(asked, ["apps"]);
    assert_eq!(last_state(&effects).mode, None);
    assert!(test.lines.get().iter().any(|line| line.contains("\"pi\"")));
}

#[test]
fn input_in_a_mode_is_read_as_the_plain_search_for_now() {
    let mut test = Test::new(vec![global("apps"), plugin("pi", "")]);
    let [apps] = test.open()[..] else { panic!() };
    test.items(apps, vec![copy("a", "Alpha"), copy("b", "Beta")]);
    let state = only_state(test.send(Incoming::Input {
        seq: 1,
        mode: Some("pi".into()),
        text: "alp".into(),
    }));
    assert_eq!(state.mode, None);
    assert_eq!(refs(&state), ["apps/a"]);
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
