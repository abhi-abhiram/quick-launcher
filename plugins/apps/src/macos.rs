//! Installed apps on macOS: the `.app` bundles in the Applications folders,
//! each launched by its path through the core's platform (`open -a`).

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use quick_launcher_plugin::{Item, Plugin};

use crate::{App, AppIcon};

/// How deep below a root folder bundles are looked for: `/Applications/X.app`
/// is depth 1, `/Applications/Utilities/X.app` or a vendor's folder of apps
/// depth 2.
const MAX_DEPTH: usize = 2;

/// Where apps are looked for, highest precedence first: the user's
/// Applications folder (when there's an absolute home), the system's, and
/// Finder, which lives elsewhere. A root may be a bundle itself.
pub fn roots(home: Option<&Path>) -> Vec<PathBuf> {
    home.filter(|home| home.is_absolute())
        .map(|home| home.join("Applications"))
        .into_iter()
        .chain([
            "/Applications".into(),
            "/System/Applications".into(),
            "/System/Library/CoreServices/Finder.app".into(),
        ])
        .collect()
}

/// [`roots`] for the process's `HOME`.
pub fn roots_from_process() -> Vec<PathBuf> {
    roots(env::var_os("HOME").map(PathBuf::from).as_deref())
}

/// Every bundle under `roots`, sorted by name. A bundle hides one with the
/// same file name, ignoring case as APFS does, under a later root, so a
/// copy in `~/Applications` wins over the system's. Folders inside bundles aren't searched, and
/// symlinked folders aren't followed (symlinked bundles are), so a link
/// cycle can't hang the walk. Unreadable folders are reported on stderr and
/// skipped.
pub fn find_bundles(roots: &[PathBuf]) -> Vec<App> {
    let mut paths = Vec::new();
    for root in roots {
        if is_bundle(root) {
            paths.push(root.clone());
        } else {
            walk(root, 1, &mut paths);
        }
    }
    let mut seen = HashSet::new();
    let mut apps: Vec<App> = paths
        .into_iter()
        .filter(|path| {
            seen.insert(
                path.file_name()
                    .map(|name| name.to_string_lossy().to_lowercase()),
            )
        })
        .filter_map(|path| app(&path))
        .collect();
    apps.sort_by(|a, b| (a.name.to_lowercase(), &a.id).cmp(&(b.name.to_lowercase(), &b.id)));
    apps
}

/// The app for a bundle: named after its folder, without `.app`.
fn app(path: &Path) -> Option<App> {
    let name = path.file_stem()?.to_str()?.to_owned();
    let id = path.to_str()?.to_owned();
    Some(App {
        id,
        name,
        detail: None,
        keywords: Vec::new(),
        icon: Some(AppIcon::Path(path.to_path_buf())),
    })
}

/// A folder (or a link to one) named `*.app`.
fn is_bundle(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "app") && path.is_dir()
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            eprintln!("apps: reading {}: {e}", dir.display());
            return;
        }
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        let hidden = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_none_or(|name| name.starts_with('.'));
        if hidden {
            continue;
        }
        if is_bundle(&path) {
            out.push(path);
        } else if depth < MAX_DEPTH && path.symlink_metadata().is_ok_and(|meta| meta.is_dir()) {
            walk(&path, depth + 1, out);
        }
    }
}

/// The plugin on macOS: lists [`find_bundles`] for its roots.
pub struct MacApps(pub Vec<PathBuf>);

impl Plugin for MacApps {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        Ok(find_bundles(&self.0)
            .into_iter()
            .map(App::into_item)
            .collect())
    }
}
