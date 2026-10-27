//! Which plugins exist and what they claim (*Plugin discovery*, *Manifest*
//! and *Errors and timeouts* in `docs/protocol.md`).
//!
//! A scan reads the bundled plugin directory, then the user's, then
//! `config.toml`. Plugins that can't be used are skipped, and every reason
//! comes back as a [`Warning`] for the caller to log, so nothing here
//! writes to the log itself.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

mod config;
mod manifest;

pub use config::{Config, PluginConfig};
pub use manifest::{Kind, Lifetime, Manifest, ManifestError, QueryItem};

use crate::action::PluginRef;
use crate::platform::Dirs;

const MANIFEST: &str = "plugin.toml";

/// Where a plugin was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Bundled,
    User,
}

/// A plugin that passed discovery and is enabled.
#[derive(Debug, Clone, PartialEq)]
pub struct Plugin {
    pub manifest: Manifest,
    /// The plugin folder, named by its id.
    pub dir: PathBuf,
    pub source: Source,
    /// The prefixes this plugin answers to: `config.toml`'s if it sets them,
    /// otherwise the manifest's, less any an earlier plugin already claimed.
    /// Can be empty after conflicts.
    pub prefixes: Vec<String>,
}

impl Plugin {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    /// The plugin's binary.
    pub fn bin(&self) -> PathBuf {
        self.dir.join(&self.manifest.bin)
    }

    /// What [`run_action`](crate::run_action) needs for this plugin's
    /// actions.
    pub fn plugin_ref(&self) -> PluginRef<'_> {
        PluginRef {
            dir: &self.dir,
            env: self.manifest.env,
        }
    }
}

/// Something a scan skipped or changed, for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// A plugin directory, or a folder in one, couldn't be read.
    Unreadable { path: PathBuf, error: String },
    /// The manifest is missing, isn't valid TOML, or breaks a rule.
    InvalidManifest { dir: PathBuf, reason: String },
    /// The manifest was written for another protocol.
    Protocol { dir: PathBuf, found: i64 },
    /// The folder's name isn't the manifest's `id`.
    FolderName { dir: PathBuf, id: String },
    /// `bin` isn't a file in the plugin folder.
    MissingBin { dir: PathBuf, bin: String },
    /// `config.toml` couldn't be used; `reason` says what was ignored.
    Config { path: PathBuf, reason: String },
    /// Two enabled plugins claim a prefix; `kept` was discovered first.
    PrefixConflict {
        prefix: String,
        kept: String,
        dropped: String,
    },
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Warning::Unreadable { path, error } => {
                write!(f, "can't read {}: {error}", path.display())
            }
            Warning::InvalidManifest { dir, reason } => {
                write!(
                    f,
                    "skipping {}: invalid {MANIFEST}: {reason}",
                    dir.display()
                )
            }
            Warning::Protocol { dir, found } => write!(
                f,
                "skipping {}: {}",
                dir.display(),
                ManifestError::Protocol(*found)
            ),
            Warning::FolderName { dir, id } => write!(
                f,
                "skipping {}: the folder must be named after the plugin id {id:?}",
                dir.display()
            ),
            Warning::MissingBin { dir, bin } => write!(
                f,
                "skipping {}: its binary {bin:?} isn't in the plugin folder",
                dir.display()
            ),
            Warning::Config { path, reason } => write!(f, "{}: {reason}", path.display()),
            Warning::PrefixConflict {
                prefix,
                kept,
                dropped,
            } => write!(
                f,
                "plugins {kept:?} and {dropped:?} both claim the prefix {prefix:?}; \
                 {kept:?} keeps it"
            ),
        }
    }
}

/// The result of one scan.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scan {
    /// Enabled plugins in discovery order.
    pub plugins: Vec<Plugin>,
    pub warnings: Vec<Warning>,
}

/// Reads every plugin and `config.toml` from `dirs`.
///
/// Discovery order is the bundled directory, then the user's, each by
/// folder name. A user plugin with a bundled plugin's id takes that
/// plugin's place in the order. A plugin that's skipped replaces nothing,
/// so a broken user override leaves the bundled plugin in use.
pub fn scan(dirs: &Dirs) -> Scan {
    let mut warnings = Vec::new();
    let mut plugins: Vec<Plugin> = Vec::new();
    for (root, source) in [
        (&dirs.bundled_plugins, Source::Bundled),
        (&dirs.user_plugins, Source::User),
    ] {
        for dir in plugin_folders(root, &mut warnings) {
            let Some(manifest) = load(&dir, &mut warnings) else {
                continue;
            };
            let plugin = Plugin {
                prefixes: manifest.prefixes.clone(),
                manifest,
                dir,
                source,
            };
            match plugins.iter_mut().find(|known| known.id() == plugin.id()) {
                Some(known) => *known = plugin,
                None => plugins.push(plugin),
            }
        }
    }

    let config = load_config(&dirs.config_file, &mut warnings);
    plugins.retain(|plugin| !config.is_disabled(plugin.id()));
    for plugin in &mut plugins {
        match config.prefixes(plugin.id()) {
            Some(prefixes)
                if prefixes.is_empty()
                    || prefixes.iter().any(|prefix| prefix.trim().is_empty()) =>
            {
                warnings.push(Warning::Config {
                    path: dirs.config_file.clone(),
                    reason: format!(
                        "plugins.{}.prefixes can't be empty or hold a blank prefix; \
                         using the manifest's",
                        plugin.id()
                    ),
                });
            }
            Some(prefixes) => plugin.prefixes = prefixes.to_vec(),
            None => {}
        }
    }
    resolve_conflicts(&mut plugins, &mut warnings);
    Scan { plugins, warnings }
}

/// Gives each prefix to the first plugin that claims it, compared without
/// case. A plugin repeating its own prefix just loses the repeat.
fn resolve_conflicts(plugins: &mut [Plugin], warnings: &mut Vec<Warning>) {
    let mut owners: HashMap<String, String> = HashMap::new();
    for plugin in plugins {
        let id = plugin.manifest.id.clone();
        plugin.prefixes.retain(|prefix| {
            let key = prefix.to_lowercase();
            match owners.get(&key) {
                None => {
                    owners.insert(key, id.clone());
                    true
                }
                Some(owner) if *owner == id => false,
                Some(owner) => {
                    warnings.push(Warning::PrefixConflict {
                        prefix: prefix.clone(),
                        kept: owner.clone(),
                        dropped: id.clone(),
                    });
                    false
                }
            }
        });
    }
}

/// The folders in a plugin directory, by name. A directory that doesn't
/// exist has none.
fn plugin_folders(root: &Path, warnings: &mut Vec<Warning>) -> Vec<PathBuf> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            warnings.push(unreadable(root, &error));
            return Vec::new();
        }
    };
    let mut folders: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    folders.sort();
    folders
}

/// The plugin in `dir`, or `None` with a warning saying why it's skipped.
fn load(dir: &Path, warnings: &mut Vec<Warning>) -> Option<Manifest> {
    let text = match fs::read_to_string(dir.join(MANIFEST)) {
        Ok(text) => text,
        Err(error) => {
            let reason = if error.kind() == io::ErrorKind::NotFound {
                "there is none".to_owned()
            } else {
                error.to_string()
            };
            warnings.push(Warning::InvalidManifest {
                dir: dir.to_path_buf(),
                reason,
            });
            return None;
        }
    };
    let manifest = match Manifest::parse(&text) {
        Ok(manifest) => manifest,
        Err(ManifestError::Protocol(found)) => {
            warnings.push(Warning::Protocol {
                dir: dir.to_path_buf(),
                found,
            });
            return None;
        }
        Err(ManifestError::Invalid(reason)) => {
            warnings.push(Warning::InvalidManifest {
                dir: dir.to_path_buf(),
                reason,
            });
            return None;
        }
    };
    if dir.file_name() != Some(manifest.id.as_ref()) {
        warnings.push(Warning::FolderName {
            dir: dir.to_path_buf(),
            id: manifest.id,
        });
        return None;
    }
    if !dir.join(&manifest.bin).is_file() {
        warnings.push(Warning::MissingBin {
            dir: dir.to_path_buf(),
            bin: manifest.bin,
        });
        return None;
    }
    Some(manifest)
}

/// `config.toml`, or the defaults when it's missing or can't be used.
fn load_config(path: &Path, warnings: &mut Vec<Warning>) -> Config {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Config::default(),
        Err(error) => {
            warnings.push(unreadable(path, &error));
            return Config::default();
        }
    };
    Config::parse(&text).unwrap_or_else(|reason| {
        warnings.push(Warning::Config {
            path: path.to_path_buf(),
            reason: format!("{reason}; ignoring the whole file"),
        });
        Config::default()
    })
}

fn unreadable(path: &Path, error: &io::Error) -> Warning {
    Warning::Unreadable {
        path: path.to_path_buf(),
        error: error.to_string(),
    }
}

/// What a scan read, by modification time and size. Two stamps differ when
/// a plugin folder was added or removed, or a directory, manifest or
/// `config.toml` changed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp(Vec<(PathBuf, Option<(SystemTime, u64)>)>);

impl Stamp {
    fn take(dirs: &Dirs) -> Stamp {
        let mut paths = vec![dirs.config_file.clone()];
        for root in [&dirs.bundled_plugins, &dirs.user_plugins] {
            paths.push(root.clone());
            for dir in plugin_folders(root, &mut Vec::new()) {
                paths.push(dir.join(MANIFEST));
                paths.push(dir);
            }
        }
        Stamp(
            paths
                .into_iter()
                .map(|path| {
                    let meta = fs::metadata(&path).ok();
                    let stamp = meta.and_then(|meta| Some((meta.modified().ok()?, meta.len())));
                    (path, stamp)
                })
                .collect(),
        )
    }
}

/// The plugins as of the last scan, rescanned when something changed.
#[derive(Debug, Clone)]
pub struct Registry {
    dirs: Dirs,
    stamp: Stamp,
    scan: Scan,
}

impl Registry {
    /// Scans `dirs` for the first time.
    pub fn new(dirs: Dirs) -> Registry {
        let stamp = Stamp::take(&dirs);
        let scan = scan(&dirs);
        Registry { dirs, stamp, scan }
    }

    /// Enabled plugins in discovery order.
    pub fn plugins(&self) -> &[Plugin] {
        &self.scan.plugins
    }

    pub fn get(&self, id: &str) -> Option<&Plugin> {
        self.plugins().iter().find(|plugin| plugin.id() == id)
    }

    /// The last scan's warnings.
    pub fn warnings(&self) -> &[Warning] {
        &self.scan.warnings
    }

    /// Scans again if a plugin directory, folder, manifest or
    /// `config.toml` changed since the last scan; the launcher does this
    /// on `open`. Returns whether it scanned, so the caller logs the new
    /// warnings only then.
    pub fn rescan_if_changed(&mut self) -> bool {
        let stamp = Stamp::take(&self.dirs);
        if stamp == self.stamp {
            return false;
        }
        // Stamp first: a change during the scan then shows up next time.
        self.stamp = stamp;
        self.scan = scan(&self.dirs);
        true
    }
}

#[cfg(test)]
mod tests;
