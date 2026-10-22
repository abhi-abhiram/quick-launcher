//! Reading desktop entry files, as the freedesktop Desktop Entry
//! Specification describes them: the `[Desktop Entry]` group, escapes in
//! values, `;`-separated lists and localized keys.

use std::collections::HashMap;

/// The `[Desktop Entry]` group of one file, keys as written (`Name[de]`
/// included) and values not yet unescaped.
#[derive(Debug, Default)]
pub struct Entry {
    keys: HashMap<String, String>,
}

impl Entry {
    /// Reads the `[Desktop Entry]` group out of a file's text. Other groups
    /// (`[Desktop Action …]`) are skipped. `None` when there's no such group.
    pub fn parse(text: &str) -> Option<Entry> {
        let mut entry = None;
        let mut in_group = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                in_group = line == "[Desktop Entry]" && entry.is_none();
                if in_group {
                    entry = Some(Entry::default());
                }
                continue;
            }
            let (Some(entry), true) = (entry.as_mut(), in_group) else {
                continue;
            };
            if let Some((key, value)) = line.split_once('=') {
                // A repeated key is invalid; the first one counts.
                entry
                    .keys
                    .entry(key.trim().to_string())
                    .or_insert_with(|| value.trim().to_string());
            }
        }
        entry
    }

    /// A string value, unescaped.
    pub fn string(&self, key: &str) -> Option<String> {
        self.keys.get(key).map(|raw| unescape(raw))
    }

    /// A localized string value: the best match for `locale`, else the
    /// unlocalized one.
    pub fn localized(&self, key: &str, locale: &Locale) -> Option<String> {
        self.localized_raw(key, locale).map(unescape)
    }

    /// A localized `;`-separated list, empty items dropped.
    pub fn localized_list(&self, key: &str, locale: &Locale) -> Vec<String> {
        self.localized_raw(key, locale)
            .map(split_list)
            .unwrap_or_default()
    }

    /// A `;`-separated list, empty items dropped.
    pub fn list(&self, key: &str) -> Vec<String> {
        self.keys
            .get(key)
            .map(|raw| split_list(raw))
            .unwrap_or_default()
    }

    /// Whether a boolean key is `true`. Missing or anything else is false.
    pub fn flag(&self, key: &str) -> bool {
        self.keys.get(key).is_some_and(|value| value == "true")
    }

    fn localized_raw(&self, key: &str, locale: &Locale) -> Option<&str> {
        locale
            .candidates()
            .iter()
            .find_map(|suffix| self.keys.get(&format!("{key}[{suffix}]")))
            .or_else(|| self.keys.get(key))
            .map(String::as_str)
    }
}

/// The messages locale, split as `lang_COUNTRY.ENCODING@MODIFIER`. The
/// encoding plays no part in matching.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Locale {
    lang: Option<String>,
    country: Option<String>,
    modifier: Option<String>,
}

impl Locale {
    /// Parses a POSIX locale name. `C`, `POSIX` and an empty name mean no
    /// translation.
    pub fn parse(name: &str) -> Locale {
        let (rest, modifier) = match name.split_once('@') {
            Some((rest, modifier)) => (rest, Some(modifier.to_string())),
            None => (name, None),
        };
        let rest = rest.split_once('.').map_or(rest, |(rest, _)| rest);
        let (lang, country) = match rest.split_once('_') {
            Some((lang, country)) => (lang, Some(country.to_string())),
            None => (rest, None),
        };
        if lang.is_empty() || lang == "C" || lang == "POSIX" {
            return Locale::default();
        }
        Locale {
            lang: Some(lang.to_string()),
            country,
            modifier,
        }
    }

    /// The `[…]` suffixes to try, best first, as the spec orders them.
    fn candidates(&self) -> Vec<String> {
        let Some(lang) = &self.lang else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let (Some(country), Some(modifier)) = (&self.country, &self.modifier) {
            out.push(format!("{lang}_{country}@{modifier}"));
        }
        if let Some(country) = &self.country {
            out.push(format!("{lang}_{country}"));
        }
        if let Some(modifier) = &self.modifier {
            out.push(format!("{lang}@{modifier}"));
        }
        out.push(lang.clone());
        out
    }
}

/// Undoes `\s`, `\n`, `\t`, `\r`, `\\` and, harmlessly outside lists, `\;`.
/// An unknown escape is kept as written.
fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(';') => out.push(';'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Splits a list on `;` that isn't escaped as `\;`, then unescapes each
/// item.
fn split_list(raw: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                current.push('\\');
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ';' => items.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    items.push(current);
    items
        .iter()
        .map(|item| unescape(item.trim()))
        .filter(|item| !item.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_desktop_entry_group_is_read() {
        let entry = Entry::parse(
            "# comment\n[Desktop Entry]\nName = Files\n\n[Desktop Action new]\nName=New Window\n",
        )
        .unwrap();
        assert_eq!(entry.string("Name").as_deref(), Some("Files"));
        assert!(Entry::parse("[Desktop Action new]\nName=x\n").is_none());
    }

    #[test]
    fn values_are_unescaped() {
        let entry = Entry::parse("[Desktop Entry]\nComment=a\\sb\\\\c\\;d\\x\n").unwrap();
        assert_eq!(entry.string("Comment").as_deref(), Some("a b\\c;d\\x"));
    }

    #[test]
    fn lists_split_on_unescaped_semicolons() {
        let entry = Entry::parse("[Desktop Entry]\nKeywords=web;a\\;b; ;net;\n").unwrap();
        assert_eq!(entry.list("Keywords"), ["web", "a;b", "net"]);
    }

    #[test]
    fn localized_keys_fall_back_in_the_spec_order() {
        let entry = Entry::parse(
            "[Desktop Entry]\nName=Plain\nName[sr]=sr\nName[sr@latin]=sr@latin\nName[sr_YU]=sr_YU\n",
        )
        .unwrap();
        let name = |locale: &str| entry.localized("Name", &Locale::parse(locale)).unwrap();
        assert_eq!(name("sr_YU.UTF-8@latin"), "sr_YU");
        assert_eq!(name("sr_ME@latin"), "sr@latin");
        assert_eq!(name("sr_ME"), "sr");
        assert_eq!(name("de_DE.UTF-8"), "Plain");
        assert_eq!(name("C.UTF-8"), "Plain");
    }

    #[test]
    fn country_and_modifier_beat_each_alone() {
        let entry =
            Entry::parse("[Desktop Entry]\nName=Plain\nName[sr_YU@latin]=both\nName[sr_YU]=c\n")
                .unwrap();
        let locale = Locale::parse("sr_YU@latin");
        assert_eq!(entry.localized("Name", &locale).unwrap(), "both");
    }
}
