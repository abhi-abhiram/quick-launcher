//! The one matcher behind *Filtering and ranking* in `docs/protocol.md`. The
//! core filters list plugins' items with it, and the SDK re-exports it so a
//! `query` plugin ranks the same way.
//!
//! Scoring keeps v0's `textScore`: exact, then prefix, then substring, then
//! in-order subsequences, tighter ones first. Matching ignores case.

use crate::Item;

/// How well a query matched one text. Lower is better.
pub type TextScore = u32;

/// Scores `text` against `query`, or `None` when it doesn't match. An empty
/// query matches everything with the best score.
///
/// Every whitespace-separated term of the query must appear in `text` as a
/// substring or, failing that, as an in-order subsequence.
pub fn score(text: &str, query: &str) -> Option<TextScore> {
    let name = text.to_lowercase();
    let needle = query.trim().to_lowercase();
    if needle.is_empty() || name == needle {
        return Some(0);
    }
    if name.starts_with(&needle) {
        return Some(1);
    }
    if name.contains(&needle) {
        return Some(2);
    }
    let chars: Vec<char> = name.chars().collect();
    let mut total: TextScore = 3;
    for term in needle.split_whitespace() {
        if name.contains(term) {
            continue;
        }
        let (first, last, len) = subsequence(&chars, term)?;
        // A tighter subsequence spans fewer characters beyond the term's own.
        total += 10 + (last - first + 1 - len) as TextScore;
    }
    Some(total)
}

/// The first and last positions of `term`'s characters found in order in
/// `chars`, and how many there are, or `None` when they aren't all there.
fn subsequence(chars: &[char], term: &str) -> Option<(usize, usize, usize)> {
    let mut cursor = 0;
    let mut first = None;
    let mut last = 0;
    let mut len = 0;
    for c in term.chars() {
        let found = cursor + chars[cursor..].iter().position(|&x| x == c)?;
        first.get_or_insert(found);
        last = found;
        cursor = found + 1;
        len += 1;
    }
    Some((first?, last, len))
}

/// Which field of an item a query matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Field {
    Label,
    Detail,
    Keyword,
}

/// How well a query matched an item. Orders best first: any label match
/// before any detail match, any detail match before any keyword match, then
/// by the text score within a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Score {
    pub field: Field,
    pub text: TextScore,
}

/// What the matcher reads from an item: its label, then its detail, then its
/// keywords.
pub trait Searchable {
    fn label(&self) -> &str;

    fn detail(&self) -> Option<&str> {
        None
    }

    fn keywords(&self) -> &[String] {
        &[]
    }
}

impl Searchable for Item {
    fn label(&self) -> &str {
        &self.label
    }

    fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    fn keywords(&self) -> &[String] {
        &self.keywords
    }
}

/// Scores an item against `query`: its label if that matches, else its
/// detail, else its best keyword. `None` when nothing matches.
pub fn item_score<T: Searchable + ?Sized>(item: &T, query: &str) -> Option<Score> {
    let on = |field, text| Some(Score { field, text });
    if let Some(text) = score(item.label(), query) {
        return on(Field::Label, text);
    }
    if let Some(text) = item.detail().and_then(|detail| score(detail, query)) {
        return on(Field::Detail, text);
    }
    let text = item
        .keywords()
        .iter()
        .filter_map(|k| score(k, query))
        .min()?;
    on(Field::Keyword, text)
}

/// One item that matched, as its index in the input and its score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub index: usize,
    pub score: Score,
}

/// The items matching `query`, best first. Ties keep the input order, so an
/// empty query returns every item in order.
///
/// Returns indices and scores rather than items so results from several
/// plugins can be merged by score.
pub fn filter<T: Searchable>(items: &[T], query: &str) -> Vec<Match> {
    let mut matches: Vec<Match> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            Some(Match {
                index,
                score: item_score(item, query)?,
            })
        })
        .collect();
    // Stable, so equal scores stay in input order.
    matches.sort_by_key(|m| m.score);
    matches
}

/// Whether `text` starts with `query`, ignoring case and the query's
/// surrounding whitespace. An empty query starts nothing, so it brings up no
/// mode hints.
pub fn starts_with(text: &str, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    !query.is_empty() && text.to_lowercase().starts_with(&query)
}

/// Whether `query` names a mode outright: it is the mode's name, or one of
/// its prefixes without a trailing `:`, ignoring case. That mode's hint then
/// comes before the global items (`pi`, `term`, `terminal`).
pub fn names_mode<S: AsRef<str>>(query: &str, name: &str, prefixes: &[S]) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return false;
    }
    name.to_lowercase() == query
        || prefixes.iter().any(|prefix| {
            let prefix = prefix.as_ref().to_lowercase();
            prefix.strip_suffix(':').unwrap_or(&prefix) == query
        })
}
