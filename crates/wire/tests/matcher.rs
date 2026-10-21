//! The matcher, against v0's cases in `tests/test_model.mjs` (`textScore`,
//! `rowScore`, `filterRows`, `modeHints`, `namesMode`) plus the keyword tier
//! v1 adds.

use quick_launcher_wire::matcher::{
    Field, Score, Searchable, TextScore, filter, item_score, names_mode, score, starts_with,
};

/// A row with only the fields the matcher reads.
struct Row {
    label: &'static str,
    detail: Option<&'static str>,
    keywords: Vec<String>,
}

impl Searchable for Row {
    fn label(&self) -> &str {
        self.label
    }

    fn detail(&self) -> Option<&str> {
        self.detail
    }

    fn keywords(&self) -> &[String] {
        &self.keywords
    }
}

fn row(label: &'static str, detail: &'static str) -> Row {
    Row {
        label,
        detail: Some(detail),
        keywords: Vec::new(),
    }
}

fn with_keywords(label: &'static str, keywords: &[&str]) -> Row {
    Row {
        label,
        detail: None,
        keywords: keywords.iter().map(|k| k.to_string()).collect(),
    }
}

fn labels(rows: &[Row], query: &str) -> Vec<&'static str> {
    filter(rows, query)
        .iter()
        .map(|m| rows[m.index].label)
        .collect()
}

fn on(field: Field, text: TextScore) -> Option<Score> {
    Some(Score { field, text })
}

#[test]
fn text_scores_prefer_exact_then_prefix_then_substring_then_subsequence() {
    assert_eq!(score("pi-launcher", ""), Some(0));
    assert_eq!(score("pi", "pi"), Some(0));
    assert_eq!(score("pi-launcher", "pi"), Some(1));
    assert_eq!(score("quick-launcher", "launch"), Some(2));
    assert!(score("quick-launcher", "qlnch").unwrap() > 2);
    assert_eq!(score("quick-launcher", "xyz"), None);
}

#[test]
fn text_scores_ignore_case_and_surrounding_whitespace() {
    assert_eq!(score("Firefox", "FIRE"), Some(1));
    assert_eq!(score("Firefox", "  fire  "), Some(1));
    assert_eq!(score("Firefox", "   "), Some(0));
}

#[test]
fn tighter_subsequences_rank_higher() {
    // "qlr" spans q…l…r in both, but "ql-r" packs it into fewer characters.
    let tight = score("ql-r", "qlr").unwrap();
    let loose = score("quick-launcher", "qlr").unwrap();
    assert!(tight < loose, "{tight} < {loose}");
    // The same values v0's textScore gives.
    assert_eq!(tight, 3 + 10 + 1);
    assert_eq!(loose, 3 + 10 + 11);
}

#[test]
fn every_term_must_match() {
    // Each term matches on its own: "launch" as a substring, "qk" as a
    // subsequence.
    assert_eq!(score("quick-launcher", "launch qk"), Some(3 + 10 + 3));
    assert_eq!(score("quick-launcher", "launch zz"), None);
    // Terms that are all substrings add nothing beyond the subsequence tier.
    assert_eq!(score("quick-launcher", "launcher quick"), Some(3));
}

#[test]
fn subsequences_count_characters_not_bytes() {
    assert_eq!(score("café-bar", "éb"), Some(3 + 10 + 1));
    assert_eq!(score("ÉCOLE", "école"), Some(0));
}

#[test]
fn a_label_match_outranks_a_detail_match() {
    let rows = [
        row("webnav", "~/Projects/webnav"),
        row("tools", "~/Projects/tools"),
        row("projects-site", "~/Work/projects-site"),
    ];
    assert_eq!(labels(&rows, "proj"), ["projects-site", "webnav", "tools"]);
    assert_eq!(item_score(&rows[0], "proj"), on(Field::Detail, 2));
    assert_eq!(item_score(&rows[2], "proj"), on(Field::Label, 1));
}

#[test]
fn a_worse_label_match_still_outranks_an_exact_detail_match() {
    let rows = [row("a", "zzz"), row("z-long-gap-z-z", "")];
    assert_eq!(labels(&rows, "zzz"), ["z-long-gap-z-z", "a"]);
}

#[test]
fn keywords_match_after_label_and_detail() {
    let rows = [
        with_keywords("Firefox", &["browser", "web"]),
        row("Chromium", "Web Browser"),
        with_keywords("Webcam", &[]),
    ];
    assert_eq!(labels(&rows, "web"), ["Webcam", "Chromium", "Firefox"]);
    assert_eq!(item_score(&rows[0], "web"), on(Field::Keyword, 0));
    assert_eq!(item_score(&rows[1], "web"), on(Field::Detail, 1));
}

#[test]
fn the_best_keyword_counts() {
    let firefox = with_keywords("Firefox", &["internet", "browser", "brow"]);
    assert_eq!(item_score(&firefox, "brow"), on(Field::Keyword, 0));
    assert_eq!(item_score(&firefox, "net"), on(Field::Keyword, 2));
    assert_eq!(item_score(&firefox, "zzz"), None);
}

#[test]
fn ties_keep_input_order() {
    let rows = [
        row("notes-b", ""),
        row("notes-a", ""),
        row("my-notes", ""),
        row("notes-c", ""),
    ];
    assert_eq!(
        labels(&rows, "notes"),
        ["notes-b", "notes-a", "notes-c", "my-notes"]
    );
}

#[test]
fn an_empty_query_returns_everything_in_order() {
    let rows = [
        row("webnav", "~/Projects/webnav"),
        row("tools", "~/Projects/tools"),
        row("projects-site", "~/Work/projects-site"),
    ];
    assert_eq!(labels(&rows, ""), ["webnav", "tools", "projects-site"]);
    assert_eq!(labels(&rows, "  "), ["webnav", "tools", "projects-site"]);
    let indices: Vec<usize> = filter(&rows, "").iter().map(|m| m.index).collect();
    assert_eq!(indices, [0, 1, 2]);
}

#[test]
fn nothing_matching_returns_nothing() {
    let rows = [row("webnav", "~/Projects/webnav")];
    assert!(filter(&rows, "zzz").is_empty());
    assert!(filter::<Row>(&[], "a").is_empty());
}

#[test]
fn wire_items_are_searchable() {
    let item: quick_launcher_wire::Item = serde_json::from_value(serde_json::json!({
        "id": "firefox.desktop", "label": "Firefox", "detail": "Web Browser",
        "keywords": ["internet"],
        "actions": [{"title": "Open", "run": {"app": "firefox.desktop"}}]
    }))
    .unwrap();
    assert_eq!(item_score(&item, "fire"), on(Field::Label, 1));
    assert_eq!(item_score(&item, "browser"), on(Field::Detail, 2));
    assert_eq!(item_score(&item, "internet"), on(Field::Keyword, 0));
}

/// v0's `modes.json`: `pi` with `pi:`, `Terminal` with `>` and `term:`.
const PI: (&str, &[&str]) = ("pi", &["pi:"]);
const TERMINAL: (&str, &[&str]) = ("Terminal", &[">", "term:"]);

/// v0's `modeHints` for one mode: its name or a prefix starts with the query.
fn hints((name, prefixes): (&str, &[&str]), query: &str) -> bool {
    starts_with(name, query) || prefixes.iter().any(|prefix| starts_with(prefix, query))
}

#[test]
fn partial_prefixes_and_names_suggest_modes() {
    assert!(hints(PI, "p") && !hints(TERMINAL, "p"));
    assert!(hints(TERMINAL, "ter") && !hints(PI, "ter"));
    assert!(!hints(PI, "") && !hints(TERMINAL, ""));
    assert!(!hints(PI, "firefox") && !hints(TERMINAL, "firefox"));
    assert!(hints(TERMINAL, ">"));
    assert!(hints(TERMINAL, " TERM "));
}

#[test]
fn only_naming_a_mode_outright_puts_its_hint_first() {
    assert!(names_mode("pi", PI.0, PI.1));
    assert!(names_mode("Terminal", TERMINAL.0, TERMINAL.1));
    assert!(names_mode("term", TERMINAL.0, TERMINAL.1));
    assert!(!names_mode("p", PI.0, PI.1));
    assert!(!names_mode("te", TERMINAL.0, TERMINAL.1));
    assert!(!names_mode("", PI.0, PI.1));
    assert!(names_mode(" PI ", PI.0, PI.1));
    assert!(names_mode(">", TERMINAL.0, TERMINAL.1));
}
