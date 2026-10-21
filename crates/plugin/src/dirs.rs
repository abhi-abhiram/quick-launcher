//! Where a plugin keeps its files (*Environment* in `docs/protocol.md`).

use std::env;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, anyhow};

/// This plugin's own folders, as the core passes them in `QL_CONFIG_DIR` and
/// `QL_STATE_DIR`. Use these rather than building paths, so the plugin works
/// on any platform the core supports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    /// Settings the user edits.
    pub config: PathBuf,
    /// What the plugin remembers, such as pins or history.
    pub state: PathBuf,
}

/// This plugin's folders, created if they don't exist yet. Fails when the
/// plugin wasn't started by the core, which sets the variables.
pub fn dirs() -> anyhow::Result<Dirs> {
    Ok(Dirs {
        config: dir("QL_CONFIG_DIR")?,
        state: dir("QL_STATE_DIR")?,
    })
}

fn dir(var: &str) -> anyhow::Result<PathBuf> {
    let path = env::var_os(var)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("{var} is not set; the launcher sets it when it starts a plugin"))?;
    fs::create_dir_all(&path).with_context(|| format!("creating {}", path.display()))?;
    Ok(path)
}
