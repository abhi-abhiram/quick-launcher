import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("install", ROOT / "install.py")
install = importlib.util.module_from_spec(spec)
spec.loader.exec_module(install)

USER_BINDINGS = 'hl.unbind("SUPER + SHIFT + P")\n'


class InstallTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.config = Path(self.temp.name)
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

    def shell(self):
        return json.loads(self.shell_json.read_text())

    def backups(self, path):
        return list(path.parent.glob(f"{path.name}.bak.*"))

    def test_install_copies_plugin_enables_it_and_binds_the_key(self):
        install.install(self.config)

        target = install.plugin_dir(self.config)
        for name in install.PLUGIN_FILES:
            self.assertEqual((target / name).read_bytes(), (ROOT / name).read_bytes())
        self.assertEqual(self.shell()["plugins"], [{"id": "abhiram.menu"}, {"id": install.PLUGIN_ID}])
        text = self.bindings.read_text()
        self.assertTrue(text.startswith(USER_BINDINGS))
        self.assertIn('hl.unbind("SUPER + ALT + SPACE")', text)
        self.assertIn(f"omarchy-shell shell toggle {install.PLUGIN_ID} '{{}}'", text)

    def test_install_removes_the_old_pi_button(self):
        install.install(self.config)

        self.assertEqual(self.shell()["bar"]["layout"]["left"], [{"id": "omarchy.workspaces"}])
        self.assertEqual(self.shell()["bar"]["layout"]["right"], [{"id": "omarchy.clock"}])
        for name in install.LEGACY_WIDGET_FILES:
            self.assertFalse((self.config / "omarchy/bar/modules" / name).exists())

    def test_install_backs_up_changed_files(self):
        original_shell, original_bindings = self.shell_json.read_text(), self.bindings.read_text()
        install.install(self.config)

        [shell_backup] = self.backups(self.shell_json)
        [bindings_backup] = self.backups(self.bindings)
        self.assertEqual(shell_backup.read_text(), original_shell)
        self.assertEqual(bindings_backup.read_text(), original_bindings)

    def test_reinstall_is_idempotent_and_asks_for_a_restart(self):
        install.install(self.config)
        shell, bindings = self.shell_json.read_text(), self.bindings.read_text()

        messages = install.install(self.config)

        self.assertEqual(self.shell_json.read_text(), shell)
        self.assertEqual(self.bindings.read_text(), bindings)
        self.assertEqual(len(self.backups(self.shell_json)), 1)
        self.assertEqual(len(self.backups(self.bindings)), 1)
        self.assertTrue(any("omarchy restart shell" in message for message in messages))
        self.assertEqual(sorted(p.name for p in install.plugin_dir(self.config).parent.iterdir()),
                         [install.PLUGIN_ID])

    def test_uninstall_restores_the_apps_menu_binding(self):
        install.install(self.config)
        install.uninstall(self.config)

        self.assertFalse(install.plugin_dir(self.config).exists())
        self.assertEqual(self.shell()["plugins"], [{"id": "abhiram.menu"}])
        self.assertEqual(self.bindings.read_text(), USER_BINDINGS)

    def test_uninstall_without_install_changes_nothing(self):
        self.bindings.write_text(USER_BINDINGS + "\n\n")
        install.uninstall(self.config)
        self.assertEqual(self.backups(self.bindings), [])
        self.assertEqual(self.backups(self.shell_json), [])

    def test_invalid_plugins_list_is_rejected_before_any_change(self):
        self.shell_json.write_text(json.dumps({"version": 1, "plugins": {}}))
        with self.assertRaises(ValueError):
            install.update_shell_config(self.config, enable=True)
        self.assertEqual(self.backups(self.shell_json), [])


if __name__ == "__main__":
    unittest.main()
