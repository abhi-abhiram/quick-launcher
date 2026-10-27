use std::sync::atomic::{AtomicUsize, Ordering};
use std::{env, fs, process};

use super::*;
use crate::action::PluginEnv;

/// A temporary home with the core's directories, removed on drop.
struct Fixture {
    root: PathBuf,
    dirs: Dirs,
}

impl Fixture {
    fn new() -> Fixture {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!("ql-discovery-{}-{n}", process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let config = root.join("config/quick-launcher");
        let dirs = Dirs {
            bundled_plugins: root.join("data/quick-launcher/plugins"),
            user_plugins: config.join("plugins"),
            config_file: config.join("config.toml"),
            plugin_config: config,
            plugin_state: root.join("state/quick-launcher"),
            home: root.clone(),
        };
        Fixture { root, dirs }
    }

    fn root(&self, source: Source) -> &Path {
        match source {
            Source::Bundled => &self.dirs.bundled_plugins,
            Source::User => &self.dirs.user_plugins,
        }
    }

    /// A plugin folder `folder` with `manifest` and a binary named `bin`.
    fn write(&self, source: Source, folder: &str, manifest: &str) -> PathBuf {
        let dir = self.root(source).join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(MANIFEST), manifest).unwrap();
        fs::write(dir.join("bin"), "").unwrap();
        dir
    }

    /// A valid plugin `id` with `prefixes`.
    fn plugin(&self, source: Source, id: &str, prefixes: &[&str]) -> PathBuf {
        self.write(source, id, &manifest(id, prefixes))
    }

    fn config(&self, text: &str) {
        fs::create_dir_all(self.dirs.config_file.parent().unwrap()).unwrap();
        fs::write(&self.dirs.config_file, text).unwrap();
    }

    fn scan(&self) -> Scan {
        scan(&self.dirs)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn manifest(id: &str, prefixes: &[&str]) -> String {
    format!("protocol = 1\nid = {id:?}\nname = {id:?}\nprefixes = {prefixes:?}\nbin = \"bin\"\n")
}

fn ids(scan: &Scan) -> Vec<&str> {
    scan.plugins.iter().map(Plugin::id).collect()
}

fn prefixes<'a>(scan: &'a Scan, id: &str) -> Vec<&'a str> {
    let plugin = scan.plugins.iter().find(|p| p.id() == id).unwrap();
    plugin.prefixes.iter().map(String::as_str).collect()
}

#[test]
fn no_directories_means_no_plugins_and_no_warnings() {
    let fixture = Fixture::new();
    assert_eq!(fixture.scan(), Scan::default());
}

#[test]
fn bundled_come_before_user_plugins_each_by_folder_name() {
    let fixture = Fixture::new();
    fixture.plugin(Source::User, "repos", &["repo:"]);
    fixture.plugin(Source::User, "calc", &["="]);
    fixture.plugin(Source::Bundled, "terminal", &[">"]);
    fixture.plugin(Source::Bundled, "apps", &["app:"]);
    let scan = fixture.scan();
    assert_eq!(ids(&scan), ["apps", "terminal", "calc", "repos"]);
    assert_eq!(scan.warnings, []);
    assert_eq!(scan.plugins[0].source, Source::Bundled);
    assert_eq!(scan.plugins[3].source, Source::User);
}

#[test]
fn a_plugin_knows_its_folder_binary_and_env() {
    let fixture = Fixture::new();
    let dir = fixture.write(
        Source::Bundled,
        "pi",
        "protocol = 1\nid = \"pi\"\nname = \"pi\"\nprefixes = [\"pi:\"]\nbin = \"bin\"\nenv = \"login\"\n",
    );
    let scan = fixture.scan();
    let plugin = &scan.plugins[0];
    assert_eq!(plugin.dir, dir);
    assert_eq!(plugin.bin(), dir.join("bin"));
    assert_eq!(plugin.plugin_ref().dir, dir);
    assert_eq!(plugin.plugin_ref().env, PluginEnv::Login);
}

#[test]
fn a_user_plugin_replaces_the_bundled_one_in_its_place() {
    let fixture = Fixture::new();
    fixture.plugin(Source::Bundled, "apps", &["app:"]);
    fixture.plugin(Source::Bundled, "pi", &["pi:"]);
    let user = fixture.plugin(Source::User, "apps", &["a:"]);
    let scan = fixture.scan();
    assert_eq!(ids(&scan), ["apps", "pi"]);
    assert_eq!(scan.plugins[0].source, Source::User);
    assert_eq!(scan.plugins[0].dir, user);
    assert_eq!(prefixes(&scan, "apps"), ["a:"]);
    assert_eq!(scan.warnings, []);
}

#[test]
fn a_broken_user_override_leaves_the_bundled_plugin() {
    let fixture = Fixture::new();
    fixture.plugin(Source::Bundled, "apps", &["app:"]);
    let user = fixture.write(Source::User, "apps", "id = \"apps\"");
    let scan = fixture.scan();
    assert_eq!(ids(&scan), ["apps"]);
    assert_eq!(scan.plugins[0].source, Source::Bundled);
    assert!(matches!(
        &scan.warnings[..],
        [Warning::InvalidManifest { dir, .. }] if *dir == user
    ));
}

#[test]
fn invalid_manifests_are_skipped_with_the_reason() {
    let fixture = Fixture::new();
    let bad = fixture.write(Source::Bundled, "bad", "protocol = 1\nid = \"bad\"\n");
    let empty = fixture.root(Source::Bundled).join("empty");
    fs::create_dir_all(&empty).unwrap();
    fixture.plugin(Source::Bundled, "good", &["g:"]);
    let scan = fixture.scan();
    assert_eq!(ids(&scan), ["good"]);
    let [first, second] = &scan.warnings[..] else {
        panic!("expected two warnings, got {:?}", scan.warnings);
    };
    assert!(
        matches!(first, Warning::InvalidManifest { dir, reason } if *dir == bad && reason.contains("name"))
    );
    assert!(
        matches!(second, Warning::InvalidManifest { dir, reason } if *dir == empty && reason.contains("none"))
    );
    assert!(first.to_string().contains("invalid plugin.toml"));
}

#[test]
fn the_folder_must_be_named_after_the_id() {
    let fixture = Fixture::new();
    let dir = fixture.write(Source::User, "my-repos", &manifest("repos", &["repo:"]));
    let scan = fixture.scan();
    assert_eq!(ids(&scan), Vec::<&str>::new());
    assert_eq!(
        scan.warnings,
        [Warning::FolderName {
            dir,
            id: "repos".into()
        }]
    );
}

#[test]
fn another_protocol_is_skipped_and_asks_for_a_rebuild() {
    let fixture = Fixture::new();
    let dir = fixture.write(
        Source::User,
        "old",
        &manifest("old", &["o:"]).replace("protocol = 1", "protocol = 0"),
    );
    let scan = fixture.scan();
    assert_eq!(scan.plugins, []);
    assert_eq!(scan.warnings, [Warning::Protocol { dir, found: 0 }]);
    assert!(scan.warnings[0].to_string().contains("rebuild"));
}

#[test]
fn a_missing_binary_is_skipped() {
    let fixture = Fixture::new();
    let dir = fixture.plugin(Source::User, "repos", &["repo:"]);
    fs::remove_file(dir.join("bin")).unwrap();
    // A folder by the binary's name isn't a binary either.
    let other = fixture.plugin(Source::User, "other", &["o:"]);
    fs::remove_file(other.join("bin")).unwrap();
    fs::create_dir(other.join("bin")).unwrap();
    let scan = fixture.scan();
    assert_eq!(scan.plugins, []);
    assert_eq!(
        scan.warnings,
        [
            Warning::MissingBin {
                dir: other,
                bin: "bin".into()
            },
            Warning::MissingBin {
                dir,
                bin: "bin".into()
            },
        ]
    );
}

#[test]
fn files_beside_plugin_folders_are_ignored() {
    let fixture = Fixture::new();
    fixture.plugin(Source::Bundled, "apps", &["app:"]);
    fs::write(fixture.root(Source::Bundled).join("README"), "").unwrap();
    let scan = fixture.scan();
    assert_eq!(ids(&scan), ["apps"]);
    assert_eq!(scan.warnings, []);
}

#[test]
fn disabled_plugins_are_skipped_and_claim_no_prefix() {
    let fixture = Fixture::new();
    fixture.plugin(Source::Bundled, "terminal", &[">"]);
    fixture.plugin(Source::User, "shell", &[">"]);
    fixture.config(r#"disabled = ["terminal", "not-installed"]"#);
    let scan = fixture.scan();
    assert_eq!(ids(&scan), ["shell"]);
    assert_eq!(prefixes(&scan, "shell"), [">"]);
    assert_eq!(scan.warnings, []);
}

#[test]
fn disabling_an_overridden_id_disables_the_override() {
    let fixture = Fixture::new();
    fixture.plugin(Source::Bundled, "apps", &["app:"]);
    fixture.plugin(Source::User, "apps", &["app:"]);
    fixture.config(r#"disabled = ["apps"]"#);
    assert_eq!(fixture.scan().plugins, []);
}

#[test]
fn config_prefixes_replace_the_manifests() {
    let fixture = Fixture::new();
    fixture.plugin(Source::Bundled, "pi", &["pi:"]);
    fixture.config("[plugins.pi]\nprefixes = [\"p:\", \"pi:\"]\n");
    let scan = fixture.scan();
    assert_eq!(prefixes(&scan, "pi"), ["p:", "pi:"]);
    // The manifest is kept as written.
    assert_eq!(scan.plugins[0].manifest.prefixes, ["pi:"]);
}

#[test]
fn blank_config_prefixes_keep_the_manifests() {
    for prefixes_toml in ["[]", "[\" \"]"] {
        let fixture = Fixture::new();
        fixture.plugin(Source::Bundled, "pi", &["pi:"]);
        fixture.config(&format!("[plugins.pi]\nprefixes = {prefixes_toml}\n"));
        let scan = fixture.scan();
        assert_eq!(prefixes(&scan, "pi"), ["pi:"]);
        assert!(matches!(
            &scan.warnings[..],
            [Warning::Config { reason, .. }] if reason.contains("plugins.pi.prefixes")
        ));
    }
}

#[test]
fn an_invalid_config_is_ignored_with_a_warning() {
    let fixture = Fixture::new();
    fixture.plugin(Source::Bundled, "terminal", &[">"]);
    fixture.config("disabled = \"terminal\"");
    let scan = fixture.scan();
    assert_eq!(ids(&scan), ["terminal"]);
    assert!(matches!(
        &scan.warnings[..],
        [Warning::Config { path, reason }]
            if *path == fixture.dirs.config_file && reason.contains("ignoring the whole file")
    ));
}

#[test]
fn the_first_plugin_discovered_keeps_a_contested_prefix() {
    let fixture = Fixture::new();
    fixture.plugin(Source::Bundled, "terminal", &[">", "term:"]);
    fixture.plugin(Source::User, "shell", &["sh:", "TERM:"]);
    let scan = fixture.scan();
    assert_eq!(prefixes(&scan, "terminal"), [">", "term:"]);
    assert_eq!(prefixes(&scan, "shell"), ["sh:"]);
    assert_eq!(
        scan.warnings,
        [Warning::PrefixConflict {
            prefix: "TERM:".into(),
            kept: "terminal".into(),
            dropped: "shell".into(),
        }]
    );
    let message = scan.warnings[0].to_string();
    assert!(message.contains("terminal") && message.contains("shell"));
}

#[test]
fn a_prefix_override_can_create_a_conflict() {
    let fixture = Fixture::new();
    fixture.plugin(Source::Bundled, "apps", &["app:"]);
    fixture.plugin(Source::Bundled, "pi", &["pi:"]);
    fixture.config("[plugins.pi]\nprefixes = [\"app:\", \"p:\"]\n");
    let scan = fixture.scan();
    assert_eq!(prefixes(&scan, "apps"), ["app:"]);
    assert_eq!(prefixes(&scan, "pi"), ["p:"]);
    assert_eq!(
        scan.warnings,
        [Warning::PrefixConflict {
            prefix: "app:".into(),
            kept: "apps".into(),
            dropped: "pi".into(),
        }]
    );
}

#[test]
fn a_plugin_repeating_its_own_prefix_is_not_a_conflict() {
    let fixture = Fixture::new();
    fixture.plugin(Source::Bundled, "pi", &["pi:", "PI:"]);
    let scan = fixture.scan();
    assert_eq!(prefixes(&scan, "pi"), ["pi:"]);
    assert_eq!(scan.warnings, []);
}

#[test]
fn a_plugin_can_lose_every_prefix_and_stay_discovered() {
    let fixture = Fixture::new();
    fixture.plugin(Source::Bundled, "apps", &["app:"]);
    fixture.plugin(Source::User, "launch", &["app:"]);
    let scan = fixture.scan();
    assert_eq!(ids(&scan), ["apps", "launch"]);
    assert_eq!(prefixes(&scan, "launch"), Vec::<&str>::new());
}

#[test]
fn rescan_does_nothing_when_nothing_changed() {
    let fixture = Fixture::new();
    fixture.plugin(Source::Bundled, "apps", &["app:"]);
    let mut registry = Registry::new(fixture.dirs.clone());
    assert!(!registry.rescan_if_changed());
    assert_eq!(registry.plugins().len(), 1);
}

#[test]
fn rescan_picks_up_an_added_and_a_removed_plugin() {
    let fixture = Fixture::new();
    let apps = fixture.plugin(Source::Bundled, "apps", &["app:"]);
    let mut registry = Registry::new(fixture.dirs.clone());
    assert_eq!(ids(&registry.scan), ["apps"]);

    fixture.plugin(Source::User, "repos", &["repo:"]);
    assert!(registry.rescan_if_changed());
    assert_eq!(ids(&registry.scan), ["apps", "repos"]);
    assert!(registry.get("repos").is_some());

    fs::remove_dir_all(apps).unwrap();
    assert!(registry.rescan_if_changed());
    assert_eq!(ids(&registry.scan), ["repos"]);
    assert!(registry.get("apps").is_none());
    assert!(!registry.rescan_if_changed());
}

#[test]
fn rescan_picks_up_a_user_directory_created_after_the_first_scan() {
    let fixture = Fixture::new();
    let mut registry = Registry::new(fixture.dirs.clone());
    assert_eq!(registry.plugins(), []);
    fixture.plugin(Source::User, "repos", &["repo:"]);
    assert!(registry.rescan_if_changed());
    assert_eq!(ids(&registry.scan), ["repos"]);
}

#[test]
fn rescan_picks_up_an_edited_manifest_and_config() {
    let fixture = Fixture::new();
    let dir = fixture.plugin(Source::Bundled, "pi", &["pi:"]);
    let mut registry = Registry::new(fixture.dirs.clone());

    // A longer manifest, so the change shows even on a coarse mtime.
    fs::write(dir.join(MANIFEST), manifest("pi", &["pi:", "p:"])).unwrap();
    assert!(registry.rescan_if_changed());
    assert_eq!(prefixes(&registry.scan, "pi"), ["pi:", "p:"]);

    fixture.config(r#"disabled = ["pi"]"#);
    assert!(registry.rescan_if_changed());
    assert_eq!(registry.plugins(), []);
}

#[test]
fn rescan_refreshes_the_warnings() {
    let fixture = Fixture::new();
    let dir = fixture.plugin(Source::User, "repos", &["repo:"]);
    fs::remove_file(dir.join("bin")).unwrap();
    let mut registry = Registry::new(fixture.dirs.clone());
    assert_eq!(registry.warnings().len(), 1);

    fs::write(dir.join("bin"), "").unwrap();
    assert!(registry.rescan_if_changed());
    assert_eq!(registry.warnings(), []);
    assert_eq!(ids(&registry.scan), ["repos"]);
}
