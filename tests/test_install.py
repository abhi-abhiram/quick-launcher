import importlib.util
import json
import os
from pathlib import Path
import plistlib
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("install", ROOT / "install.py")
install = importlib.util.module_from_spec(spec)
spec.loader.exec_module(install)

USER_BINDINGS = 'hl.unbind("SUPER + SHIFT + P")\n'


def fake_binaries(folder: Path) -> Path:
    """Stand-ins for the release build: the core and each bundled plugin's
    binary, each holding its own name."""
    folder.mkdir(parents=True)
    names = [install.CORE_BIN] + [m["bin"] for _, m in install.plugin_layout.bundled_plugins()]
    for name in names:
        (folder / name).write_text(name)
    return folder


class InstallTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.config = Path(self.temp.name) / "config"
        self.data = Path(self.temp.name) / "data"
        self.bin = fake_binaries(Path(self.temp.name) / "release")
        (self.config / "omarchy/bar/modules").mkdir(parents=True)
        (self.config / "hypr").mkdir()
        self.shell_json.write_text(json.dumps({
            "version": 1,
            "bar": {"layout": {
                "left": [{"id": "omarchy.workspaces"}, {"id": "pi-launcher", "type": "qml"}],
                "right": [{"id": "omarchy.clock"}],
            }},
            "plugins": [{"id": "abhiram.menu"}],
        }))
        self.bindings.write_text(USER_BINDINGS)
        for name in install.LEGACY_WIDGET_FILES:
            (self.config / "omarchy/bar/modules" / name).write_text("old")

    def tearDown(self):
        self.temp.cleanup()

    @property
    def shell_json(self):
        return self.config / "omarchy/shell.json"

    @property
    def bindings(self):
        return self.config / "hypr/bindings.lua"

    def install(self):
        return install.install(self.config, self.data, self.bin)

    def shell(self):
        return json.loads(self.shell_json.read_text())

    def backups(self, path):
        return list(path.parent.glob(f"{path.name}.bak.*"))

    def test_install_copies_plugin_and_enables_it(self):
        self.install()

        target = install.plugin_dir(self.config)
        self.assertEqual(sorted(p.name for p in target.iterdir()), sorted(install.PLUGIN_FILES))
        for name in install.PLUGIN_FILES:
            self.assertEqual((target / name).read_bytes(), (ROOT / "frontends/omarchy" / name).read_bytes())
        self.assertEqual(self.shell()["plugins"], [{"id": "abhiram.menu"}, {"id": install.PLUGIN_ID}])

    def test_install_puts_the_core_and_bundled_plugins_under_the_data_home(self):
        self.install()

        core = self.data / "quick-launcher/bin/quick-launcher-core"
        self.assertEqual(core.read_text(), "quick-launcher-core")
        self.assertTrue(core.stat().st_mode & 0o111)
        plugins = self.data / "quick-launcher/plugins"
        self.assertEqual(sorted(p.name for p in plugins.iterdir()), ["apps", "pi", "projects", "terminal"])
        for _, manifest in install.plugin_layout.bundled_plugins():
            folder = plugins / manifest["id"]
            self.assertEqual(sorted(p.name for p in folder.iterdir()), sorted(["plugin.toml", manifest["bin"]]))
            self.assertEqual((folder / manifest["bin"]).read_text(), manifest["bin"])
            self.assertEqual((folder / "plugin.toml").read_bytes(),
                             (ROOT / "plugins" / manifest["id"] / "plugin.toml").read_bytes())

    def test_the_core_finds_the_installed_plugins_where_the_launcher_starts_it(self):
        # Launcher.qml starts $XDG_DATA_HOME/quick-launcher/bin/quick-launcher-core,
        # and the core's Omarchy platform reads $XDG_DATA_HOME/quick-launcher/plugins.
        qml = (ROOT / "frontends/omarchy/Launcher.qml").read_text()
        self.assertIn('"/quick-launcher/bin/quick-launcher-core"', qml)
        self.assertEqual(install.core_path(self.data), self.data / "quick-launcher/bin/quick-launcher-core")
        self.assertEqual(install.bundled_plugins_dir(self.data), self.data / "quick-launcher/plugins")

    def test_reinstall_replaces_the_bundled_plugins_whole(self):
        self.install()
        stale = self.data / "quick-launcher/plugins/old-plugin"
        stale.mkdir()
        (self.bin / install.CORE_BIN).write_text("new core")

        self.install()

        self.assertFalse(stale.exists())
        self.assertEqual((self.data / "quick-launcher/bin/quick-launcher-core").read_text(), "new core")
        self.assertEqual(sorted(p.name for p in (self.data / "quick-launcher").iterdir()), ["bin", "plugins"])

    def test_a_missing_binary_fails_before_the_shell_plugin_changes(self):
        (self.bin / "pi-plugin").unlink()
        with self.assertRaises(FileNotFoundError):
            self.install()
        self.assertFalse(install.plugin_dir(self.config).exists())
        self.assertEqual(self.backups(self.shell_json), [])
        self.assertFalse(install.core_path(self.data).exists())

    def test_a_missing_plugin_binary_keeps_the_installed_core_and_plugins(self):
        self.install()
        installed = sorted(p.relative_to(self.data) for p in self.data.rglob("*"))
        (self.bin / install.CORE_BIN).write_text("new core")
        (self.bin / "pi-plugin").unlink()

        with self.assertRaises(FileNotFoundError):
            self.install()

        self.assertEqual(install.core_path(self.data).read_text(), "quick-launcher-core")
        self.assertEqual(sorted(p.relative_to(self.data) for p in self.data.rglob("*")), installed)

    def test_upgrading_from_v0_drops_its_files(self):
        old = install.plugin_dir(self.config)
        (old / "modes").mkdir(parents=True)
        for name in ("Launcher.qml", "LauncherModel.js", "modes.json", "modes/pi.py"):
            (old / name).write_text("v0")

        messages = self.install()

        self.assertEqual(sorted(p.name for p in old.iterdir()), sorted(install.PLUGIN_FILES))
        self.assertTrue(any("omarchy restart shell" in message for message in messages))

    def test_build_asks_cargo_for_the_core_and_every_bundled_plugin(self):
        with mock.patch.object(install.subprocess, "run") as run, mock.patch("builtins.print"):
            bin_dir = install.build("cargo")

        [(command,), kwargs] = run.call_args
        self.assertEqual(command[:6], ["cargo", "build", "--release", "--locked", "--target-dir", str(ROOT / "target")])
        self.assertEqual(command[6::2], ["--bin"] * 5)
        self.assertEqual(command[7::2], ["quick-launcher-core", "apps", "pi-plugin", "projects-plugin", "terminal-plugin"])
        self.assertEqual(kwargs, {"cwd": ROOT, "check": True})
        self.assertEqual(bin_dir, ROOT / "target/release")

    def test_build_uses_cargo_target_dir(self):
        for value, expected in [("/tmp/ql-target", Path("/tmp/ql-target")), ("out", ROOT / "out"), ("", ROOT / "target")]:
            with mock.patch.object(install.subprocess, "run") as run, mock.patch("builtins.print"):
                bin_dir = install.build("cargo", environ={"CARGO_TARGET_DIR": value})
            self.assertEqual(run.call_args.args[0][5], str(expected))
            self.assertEqual(bin_dir, expected / "release")

    def test_install_without_cargo_says_how_to_get_it(self):
        with mock.patch.object(install.shutil, "which", return_value=None):
            with self.assertRaisesRegex(FileNotFoundError, "cargo is not installed"):
                install.check_cargo()

    def test_install_leaves_bindings_alone_and_suggests_one(self):
        messages = self.install()

        self.assertEqual(self.bindings.read_text(), USER_BINDINGS)
        self.assertIn(install.BINDING_HINT, messages)

    def test_no_binding_hint_once_a_key_is_bound(self):
        self.bindings.write_text(f'o.bind("SUPER + SPACE", "Quick launcher", "{install.TOGGLE_COMMAND}")\n')
        self.assertNotIn(install.BINDING_HINT, self.install())

    def test_install_removes_the_old_pi_button(self):
        self.install()

        self.assertEqual(self.shell()["bar"]["layout"]["left"], [{"id": "omarchy.workspaces"}])
        self.assertEqual(self.shell()["bar"]["layout"]["right"], [{"id": "omarchy.clock"}])
        for name in install.LEGACY_WIDGET_FILES:
            self.assertFalse((self.config / "omarchy/bar/modules" / name).exists())

    def test_install_backs_up_changed_files(self):
        original_shell = self.shell_json.read_text()
        self.install()

        [shell_backup] = self.backups(self.shell_json)
        self.assertEqual(shell_backup.read_text(), original_shell)

    def test_reinstall_is_idempotent_and_asks_for_a_restart(self):
        self.install()
        shell = self.shell_json.read_text()

        messages = self.install()

        self.assertEqual(self.shell_json.read_text(), shell)
        self.assertEqual(len(self.backups(self.shell_json)), 1)
        self.assertTrue(any("omarchy restart shell" in message for message in messages))
        self.assertEqual(sorted(p.name for p in install.plugin_dir(self.config).parent.iterdir()),
                         [install.PLUGIN_ID])

    def test_uninstall_removes_and_disables_the_plugin(self):
        self.install()
        install.uninstall(self.config, self.data)

        self.assertFalse(install.plugin_dir(self.config).exists())
        self.assertEqual(self.shell()["plugins"], [{"id": "abhiram.menu"}])
        self.assertFalse((self.data / "quick-launcher").exists())

    def test_uninstall_keeps_the_users_own_files(self):
        self.install()
        user = self.data / "quick-launcher/notes.txt"
        user.write_text("mine")

        install.uninstall(self.config, self.data)

        self.assertEqual(user.read_text(), "mine")
        self.assertFalse((self.data / "quick-launcher/plugins").exists())
        self.assertFalse((self.data / "quick-launcher/bin").exists())

    def test_uninstall_reminds_about_a_leftover_binding(self):
        self.bindings.write_text(f'o.bind("SUPER + SPACE", "Quick launcher", "{install.TOGGLE_COMMAND}")\n')
        messages = install.uninstall(self.config, self.data)
        self.assertTrue(any("bindings.lua" in message for message in messages))

    def test_uninstall_without_install_changes_nothing(self):
        install.uninstall(self.config, self.data)
        self.assertEqual(self.backups(self.shell_json), [])

    def test_invalid_plugins_list_is_rejected_before_any_change(self):
        self.shell_json.write_text(json.dumps({"version": 1, "plugins": {}}))
        with self.assertRaises(ValueError):
            install.update_shell_config(self.config, enable=True)
        self.assertEqual(self.backups(self.shell_json), [])


class MacInstallTest(unittest.TestCase):
    """The macOS install: the same core and plugins, plus the app bundle.
    Nothing here needs a Mac; building and signing are left out."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        root = Path(self.temp.name)
        self.data = root / "data"
        self.apps = root / "Applications"
        self.bin = fake_binaries(root / "release")
        self.executable = root / "swift/QuickLauncher"
        self.executable.parent.mkdir()
        self.executable.write_text("app")

    def tearDown(self):
        self.temp.cleanup()

    def install(self, version="1.2.3"):
        return install.install_macos(self.data, self.apps, self.bin, self.executable, version)

    @property
    def app(self):
        return self.apps / "Quick Launcher.app"

    def test_install_puts_the_core_plugins_and_app_in_place(self):
        messages = self.install()
        self.assertEqual((self.data / "quick-launcher/bin/quick-launcher-core").read_text(), "quick-launcher-core")
        self.assertTrue((self.data / "quick-launcher/plugins/apps/plugin.toml").is_file())
        executable = self.app / "Contents/MacOS/QuickLauncher"
        self.assertEqual(executable.read_text(), "app")
        self.assertTrue(os.access(executable, os.X_OK))
        self.assertTrue(any("Option+Space" in message for message in messages))

    def test_the_app_is_a_menu_bar_app_that_opens_its_urls(self):
        self.install("1.2.3")
        with open(self.app / "Contents/Info.plist", "rb") as stream:
            info = plistlib.load(stream)
        self.assertEqual(info["CFBundleExecutable"], "QuickLauncher")
        self.assertEqual(info["CFBundleIdentifier"], install.MAC_BUNDLE_ID)
        self.assertEqual(info["CFBundleShortVersionString"], "1.2.3")
        self.assertIs(info["LSUIElement"], True)
        self.assertEqual(info["CFBundleURLTypes"][0]["CFBundleURLSchemes"], ["quick-launcher"])

    def test_a_pre_release_version_becomes_a_valid_bundle_version(self):
        for version, expected in [("0.2.0-rc1", "0.2.0"), ("1.2.3", "1.2.3"), ("2", "2"), ("dev", "0")]:
            self.assertEqual(install.info_plist(version)["CFBundleVersion"], expected, version)
            self.assertEqual(install.info_plist(version)["CFBundleShortVersionString"], expected, version)

    def test_a_failed_swap_puts_the_old_app_back(self):
        self.install()
        real_rename = Path.rename

        def rename(path, target):
            if path.name == "Quick Launcher.app" and path.parent != self.apps:
                raise OSError("disk full")
            return real_rename(path, target)

        self.executable.write_text("new app")
        with mock.patch.object(Path, "rename", rename), self.assertRaises(OSError):
            self.install()
        self.assertEqual((self.app / "Contents/MacOS/QuickLauncher").read_text(), "app")
        self.assertEqual(sorted(path.name for path in self.apps.iterdir()), ["Quick Launcher.app"])

    def test_reinstall_replaces_the_app_whole(self):
        self.install()
        (self.app / "Contents/leftover").write_text("old")
        self.executable.write_text("new app")
        self.install()
        self.assertFalse((self.app / "Contents/leftover").exists())
        self.assertEqual((self.app / "Contents/MacOS/QuickLauncher").read_text(), "new app")
        self.assertEqual(sorted(path.name for path in self.apps.iterdir()), ["Quick Launcher.app"])

    def test_a_missing_executable_fails_before_the_app_changes(self):
        self.install()
        self.executable.unlink()
        with self.assertRaises(FileNotFoundError):
            self.install()
        self.assertEqual((self.app / "Contents/MacOS/QuickLauncher").read_text(), "app")

    def test_uninstall_removes_the_app_core_and_plugins(self):
        self.install()
        with mock.patch.object(install.subprocess, "run"):
            messages = install.uninstall_macos(self.data, self.apps)
        self.assertFalse(self.app.exists())
        self.assertFalse((self.data / "quick-launcher").exists())
        self.assertTrue(any("Login Items" in message for message in messages))

    def test_the_app_builds_with_swiftpm(self):
        shown = mock.Mock(stdout="/src/frontends/macos/.build/release\n")
        with mock.patch.object(install.subprocess, "run", return_value=shown) as run, \
                mock.patch("builtins.print"):
            executable = install.build_mac_app("swift")
        package = str(ROOT / "frontends/macos")
        self.assertEqual(run.call_args_list[0].args[0], ["swift", "build", "-c", "release", "--package-path", package])
        self.assertEqual(executable, Path("/src/frontends/macos/.build/release/QuickLauncher"))

    def test_the_version_comes_from_the_workspace(self):
        self.assertRegex(install.workspace_version(), r"^\d+\.\d+\.\d+")

    def test_install_without_swift_says_how_to_get_it(self):
        with mock.patch.object(install.shutil, "which", return_value=None):
            with self.assertRaisesRegex(FileNotFoundError, "xcode-select --install"):
                install.check_swift()


if __name__ == "__main__":
    unittest.main()
