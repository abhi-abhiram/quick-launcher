//! `find_bundles` over temporary folders laid out like macOS's
//! Applications folders.

use std::fs;
use std::path::{Path, PathBuf};

use quick_launcher_apps::macos::{find_bundles, roots};
use quick_launcher_apps::{App, AppIcon};

struct Root(PathBuf);

impl Root {
    fn new(test: &str) -> Root {
        let root = std::env::temp_dir().join(format!("ql-apps-mac-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Root(root)
    }

    /// Makes the folder `path` below the root, with a file inside as a
    /// bundle has.
    fn bundle(&self, path: &str) -> PathBuf {
        let bundle = self.0.join(path);
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        fs::write(bundle.join("Contents/Info.plist"), "").unwrap();
        bundle
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn names(apps: &[App]) -> Vec<&str> {
    apps.iter().map(|app| app.name.as_str()).collect()
}

#[test]
fn bundles_are_listed_by_name_with_their_path_as_id_and_icon() {
    let root = Root::new("listed");
    let safari = root.bundle("system/Safari.app");
    root.bundle("system/Utilities/Terminal.app");
    root.bundle("system/Adobe/Tools/Too Deep.app");
    root.bundle("system/Safari.app/Contents/Inner.app");
    root.bundle("system/.Hidden.app");
    fs::create_dir_all(root.0.join("system/NotAnApp")).unwrap();
    fs::write(root.0.join("system/file.app"), "").unwrap();

    let apps = find_bundles(&[root.0.join("system")]);
    assert_eq!(names(&apps), ["Safari", "Terminal"]);
    assert_eq!(
        apps[0],
        App {
            id: safari.to_str().unwrap().into(),
            name: "Safari".into(),
            detail: None,
            keywords: vec![],
            icon: Some(AppIcon::Path(safari.clone())),
        }
    );
}

#[test]
fn an_earlier_root_hides_a_bundle_with_the_same_name() {
    let root = Root::new("precedence");
    let mine = root.bundle("user/Firefox.app");
    root.bundle("system/Firefox.app");
    root.bundle("system/Notes.app");
    let apps = find_bundles(&[root.0.join("user"), root.0.join("system")]);
    assert_eq!(names(&apps), ["Firefox", "Notes"]);
    assert_eq!(apps[0].id, mine.to_str().unwrap());
}

#[test]
fn a_root_can_be_a_bundle_and_missing_roots_are_skipped() {
    let root = Root::new("bundle-root");
    let finder = root.bundle("core/Finder.app");
    let apps = find_bundles(&[root.0.join("missing"), finder.clone()]);
    assert_eq!(names(&apps), ["Finder"]);
}

#[test]
fn symlinked_bundles_are_listed_but_symlinked_folders_are_not_walked() {
    let root = Root::new("links");
    let real = root.bundle("elsewhere/Linked.app");
    root.bundle("elsewhere/Inside.app");
    fs::create_dir_all(root.0.join("apps")).unwrap();
    std::os::unix::fs::symlink(&real, root.0.join("apps/Linked.app")).unwrap();
    std::os::unix::fs::symlink(root.0.join("elsewhere"), root.0.join("apps/folder")).unwrap();
    std::os::unix::fs::symlink("..", root.0.join("apps/loop")).unwrap();
    let apps = find_bundles(&[root.0.join("apps")]);
    assert_eq!(names(&apps), ["Linked"]);
}

#[test]
fn the_users_applications_come_first() {
    let roots = roots(Path::new("/Users/me"));
    assert_eq!(roots[0], Path::new("/Users/me/Applications"));
    assert!(roots.contains(&PathBuf::from("/Applications")));
}

#[cfg(target_os = "macos")]
#[test]
fn the_binary_lists_the_users_bundles() {
    use serde_json::{Value, json};
    use std::io::Write;
    use std::process::{Command, Stdio};

    let root = Root::new("binary");
    let bundle = root.bundle("Applications/Quick Test App.app");
    let mut child = Command::new(env!("CARGO_BIN_EXE_apps"))
        .env("HOME", &root.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"v\":1,\"id\":1,\"method\":\"list\",\"params\":{}}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    let path = bundle.to_str().unwrap();
    let item = response["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == path)
        .expect("the bundle is listed");
    assert_eq!(
        item,
        &json!({"id": path, "label": "Quick Test App", "icon": {"path": path},
                "actions": [{"id": "launch", "title": "Launch", "run": {"app": path}}]})
    );
}
