//! `find_apps` over temporary data dirs, and the built binary answering a
//! `list` request.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use quick_launcher_apps::{App, AppIcon, Env, Locale, find_apps};
use serde_json::{Value, json};

/// A user and a system data dir under a fresh temporary folder, removed on
/// drop.
struct Dirs {
    root: PathBuf,
}

impl Dirs {
    fn new(test: &str) -> Dirs {
        let root = std::env::temp_dir().join(format!("ql-apps-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Dirs { root }
    }

    fn user(&self) -> PathBuf {
        self.root.join("user")
    }

    fn system(&self) -> PathBuf {
        self.root.join("system")
    }

    fn env(&self) -> Env {
        Env {
            data_dirs: vec![self.user(), self.system()],
            ..Env::default()
        }
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Writes `applications/<file>` in `data_dir`.
fn entry(data_dir: &Path, file: &str, body: &str) {
    let path = data_dir.join("applications").join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("[Desktop Entry]\nType=Application\n{body}")).unwrap();
}

fn ids(apps: &[App]) -> Vec<&str> {
    apps.iter().map(|app| app.id.as_str()).collect()
}

#[test]
fn an_app_has_its_name_detail_keywords_and_icon() {
    let dirs = Dirs::new("fields");
    entry(
        &dirs.system(),
        "firefox.desktop",
        "Name=Firefox\nGenericName=Web Browser\nComment=Browse the Web\n\
         Keywords=Internet;WWW;\nIcon=firefox\nExec=firefox %u\n",
    );
    assert_eq!(
        find_apps(&dirs.env()),
        [App {
            id: "firefox.desktop".into(),
            name: "Firefox".into(),
            detail: Some("Browse the Web".into()),
            keywords: vec!["Internet".into(), "WWW".into()],
            icon: Some(AppIcon::Name("firefox".into())),
        }]
    );
}

#[test]
fn detail_falls_back_to_generic_name() {
    let dirs = Dirs::new("detail");
    entry(&dirs.system(), "a.desktop", "Name=A\nGenericName=Editor\n");
    entry(&dirs.system(), "b.desktop", "Name=B\n");
    entry(
        &dirs.system(),
        "c.desktop",
        "Name=C\nComment=\nGenericName=Viewer\n",
    );
    let apps = find_apps(&dirs.env());
    assert_eq!(apps[0].detail.as_deref(), Some("Editor"));
    assert_eq!(apps[1].detail, None);
    assert_eq!(apps[2].detail.as_deref(), Some("Viewer"));
}

#[test]
fn a_user_entry_hides_the_system_one_with_the_same_id() {
    let dirs = Dirs::new("precedence");
    entry(&dirs.system(), "editor.desktop", "Name=System Editor\n");
    entry(&dirs.user(), "editor.desktop", "Name=My Editor\n");
    entry(&dirs.system(), "other.desktop", "Name=Other\n");
    let apps = find_apps(&dirs.env());
    assert_eq!(ids(&apps), ["editor.desktop", "other.desktop"]);
    assert_eq!(apps[0].name, "My Editor");
}

#[test]
fn an_unreadable_user_entry_does_not_hide_the_system_one() {
    let dirs = Dirs::new("unreadable");
    entry(&dirs.system(), "editor.desktop", "Name=Editor\n");
    fs::create_dir_all(dirs.user().join("applications")).unwrap();
    std::os::unix::fs::symlink(
        dirs.root.join("missing"),
        dirs.user().join("applications/editor.desktop"),
    )
    .unwrap();
    assert_eq!(ids(&find_apps(&dirs.env())), ["editor.desktop"]);
}

#[test]
fn symlinked_files_are_read_but_symlinked_folders_are_not_followed() {
    let dirs = Dirs::new("symlinks");
    entry(&dirs.system(), "a/real.desktop", "Name=Real\n");
    // A cycle: a/loop -> a. Following it would never end.
    std::os::unix::fs::symlink("..", dirs.system().join("applications/a/loop")).unwrap();
    entry(&dirs.root, "elsewhere.desktop", "Name=Linked\n");
    std::os::unix::fs::symlink(
        dirs.root.join("applications/elsewhere.desktop"),
        dirs.system().join("applications/linked.desktop"),
    )
    .unwrap();
    assert_eq!(
        ids(&find_apps(&dirs.env())),
        ["linked.desktop", "a-real.desktop"]
    );
}

#[test]
fn hidden_and_no_display_entries_are_skipped() {
    let dirs = Dirs::new("hidden");
    entry(
        &dirs.system(),
        "nodisplay.desktop",
        "Name=A\nNoDisplay=true\n",
    );
    entry(&dirs.system(), "hidden.desktop", "Name=B\nHidden=true\n");
    entry(&dirs.system(), "shown.desktop", "Name=C\nNoDisplay=false\n");
    // A hidden user entry removes the system app, as a user deleting it
    // from menus does.
    entry(&dirs.system(), "removed.desktop", "Name=D\n");
    entry(&dirs.user(), "removed.desktop", "Name=D\nHidden=true\n");
    assert_eq!(ids(&find_apps(&dirs.env())), ["shown.desktop"]);
}

#[test]
fn entries_that_are_not_apps_or_lack_a_name_are_skipped() {
    let dirs = Dirs::new("invalid");
    let link = dirs.system().join("applications/link.desktop");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    fs::write(
        &link,
        "[Desktop Entry]\nType=Link\nName=Site\nURL=https://x\n",
    )
    .unwrap();
    fs::write(
        dirs.system().join("applications/no-group.desktop"),
        "Name=X\n",
    )
    .unwrap();
    fs::write(dirs.system().join("applications/notes.txt"), "").unwrap();
    entry(&dirs.system(), "nameless.desktop", "Exec=true\n");
    entry(&dirs.system(), "app.desktop", "Name=App\n");
    assert_eq!(ids(&find_apps(&dirs.env())), ["app.desktop"]);
}

#[test]
fn ids_of_entries_in_subfolders_join_the_path_with_dashes() {
    let dirs = Dirs::new("subdir");
    entry(&dirs.system(), "kde/konsole.desktop", "Name=Konsole\n");
    assert_eq!(ids(&find_apps(&dirs.env())), ["kde-konsole.desktop"]);
}

#[test]
fn localized_keys_fall_back_to_the_language_then_the_plain_key() {
    let dirs = Dirs::new("locale");
    entry(
        &dirs.system(),
        "files.desktop",
        "Name=Files\nName[de]=Dateien\nName[de_AT]=Dateien (AT)\n\
         Comment=Browse files\nComment[de]=Dateien durchsuchen\n\
         Keywords=folder;\nKeywords[de]=Ordner;\n",
    );
    let with = |locale: &str| {
        let env = Env {
            locale: Locale::parse(locale),
            ..dirs.env()
        };
        find_apps(&env).remove(0)
    };

    let at = with("de_AT.UTF-8");
    assert_eq!(at.name, "Dateien (AT)");
    assert_eq!(at.detail.as_deref(), Some("Dateien durchsuchen"));
    assert_eq!(at.keywords, ["Ordner"]);

    assert_eq!(with("de_CH.UTF-8").name, "Dateien");
    let french = with("fr_FR.UTF-8");
    assert_eq!(french.name, "Files");
    assert_eq!(french.keywords, ["folder"]);
    assert_eq!(with("C").name, "Files");
}

#[test]
fn icons_are_names_or_absolute_paths() {
    let dirs = Dirs::new("icons");
    entry(
        &dirs.system(),
        "a.desktop",
        "Name=A\nIcon=utilities-terminal\n",
    );
    entry(
        &dirs.system(),
        "b.desktop",
        "Name=B\nIcon=/opt/b/icon.png\n",
    );
    entry(&dirs.system(), "c.desktop", "Name=C\nIcon=\n");
    entry(&dirs.system(), "d.desktop", "Name=D\n");
    let icons: Vec<_> = find_apps(&dirs.env())
        .into_iter()
        .map(|app| app.icon)
        .collect();
    assert_eq!(
        icons,
        [
            Some(AppIcon::Name("utilities-terminal".into())),
            Some(AppIcon::Path("/opt/b/icon.png".into())),
            None,
            None,
        ]
    );
}

#[test]
fn only_show_in_and_not_show_in_follow_the_current_desktop() {
    let dirs = Dirs::new("desktops");
    entry(
        &dirs.system(),
        "gnome.desktop",
        "Name=G\nOnlyShowIn=GNOME;\n",
    );
    entry(
        &dirs.system(),
        "hypr.desktop",
        "Name=H\nOnlyShowIn=GNOME;Hyprland;\n",
    );
    entry(
        &dirs.system(),
        "not.desktop",
        "Name=N\nNotShowIn=Hyprland;\n",
    );
    entry(&dirs.system(), "any.desktop", "Name=A\n");
    let env = Env {
        desktops: vec!["Hyprland".into()],
        ..dirs.env()
    };
    assert_eq!(ids(&find_apps(&env)), ["any.desktop", "hypr.desktop"]);
}

#[test]
fn the_first_current_desktop_named_in_either_list_decides() {
    let dirs = Dirs::new("desktop-order");
    entry(
        &dirs.system(),
        "a.desktop",
        "Name=A\nOnlyShowIn=ubuntu;\nNotShowIn=GNOME;\n",
    );
    entry(
        &dirs.system(),
        "b.desktop",
        "Name=B\nOnlyShowIn=GNOME;\nNotShowIn=ubuntu;\n",
    );
    entry(&dirs.system(), "c.desktop", "Name=C\nOnlyShowIn=KDE;\n");
    let env = Env {
        desktops: vec!["ubuntu".into(), "GNOME".into()],
        ..dirs.env()
    };
    assert_eq!(ids(&find_apps(&env)), ["a.desktop"]);
}

#[test]
fn try_exec_must_name_an_installed_program() {
    let dirs = Dirs::new("tryexec");
    let bin = dirs.root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("present"), "").unwrap();
    fs::set_permissions(bin.join("present"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(bin.join("plain"), "").unwrap();
    fs::set_permissions(bin.join("plain"), fs::Permissions::from_mode(0o644)).unwrap();
    entry(&dirs.system(), "d.desktop", "Name=D\nTryExec=plain\n");
    entry(&dirs.system(), "a.desktop", "Name=A\nTryExec=present\n");
    entry(&dirs.system(), "b.desktop", "Name=B\nTryExec=absent\n");
    let absolute = format!("Name=C\nTryExec={}\n", bin.join("present").display());
    entry(&dirs.system(), "c.desktop", &absolute);
    let env = Env {
        path: vec![bin],
        ..dirs.env()
    };
    assert_eq!(ids(&find_apps(&env)), ["a.desktop", "c.desktop"]);
}

#[test]
fn apps_are_sorted_by_name_ignoring_case() {
    let dirs = Dirs::new("sorted");
    entry(&dirs.system(), "1.desktop", "Name=zed\n");
    entry(&dirs.system(), "2.desktop", "Name=Alacritty\n");
    entry(&dirs.user(), "3.desktop", "Name=btop\n");
    let names: Vec<_> = find_apps(&dirs.env())
        .into_iter()
        .map(|app| app.name)
        .collect();
    assert_eq!(names, ["Alacritty", "btop", "zed"]);
}

#[test]
fn the_binary_answers_a_list_request_with_launch_actions() {
    let dirs = Dirs::new("binary");
    entry(
        &dirs.user(),
        "firefox.desktop",
        "Name=Firefox\nComment=Browse the Web\nIcon=firefox\nKeywords=web;\n",
    );
    entry(
        &dirs.system(),
        "tool.desktop",
        "Name=Tool\nIcon=/opt/tool.svg\n",
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_apps"))
        .env("XDG_DATA_HOME", dirs.user())
        .env("XDG_DATA_DIRS", dirs.system())
        .env("LANG", "C.UTF-8")
        .env_remove("LC_ALL")
        .env_remove("LC_MESSAGES")
        .env_remove("XDG_CURRENT_DESKTOP")
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

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "one response line: {stdout}");
    let response: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(
        response,
        json!({"v": 1, "id": 1, "items": [
            {"id": "firefox.desktop", "label": "Firefox", "detail": "Browse the Web",
             "icon": {"name": "firefox"}, "keywords": ["web"],
             "actions": [{"id": "launch", "title": "Launch", "run": {"app": "firefox.desktop"}}]},
            {"id": "tool.desktop", "label": "Tool", "icon": {"path": "/opt/tool.svg"},
             "actions": [{"id": "launch", "title": "Launch", "run": {"app": "tool.desktop"}}]},
        ]})
    );
}
