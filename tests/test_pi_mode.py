import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parent.parent


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "modes" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


pi = load("pi")


def run_list(module, **kwargs):
    with mock.patch("sys.stdout", new=io.StringIO()) as out:
        module.main(["list"], **kwargs)
    return json.loads(out.getvalue())["rows"]


class PiModeTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.home = self.root / "home"
        self.sessions = self.root / "sessions"
        self.home.mkdir()
        self.sessions.mkdir()
        self.environ = {
            "XDG_CONFIG_HOME": str(self.root / "config"),
            "PI_CODING_AGENT_SESSION_DIR": str(self.sessions),
        }

    def tearDown(self):
        self.temp.cleanup()

    def project(self, name):
        path = self.home / name
        path.mkdir()
        return str(path)

    def session(self, cwd, mtime, group="group"):
        folder = self.sessions / group
        folder.mkdir(exist_ok=True)
        file = folder / f"{mtime}.jsonl"
        file.write_text(json.dumps({"type": "session", "cwd": cwd}) + "\n")
        os.utime(file, (mtime, mtime))

    def rows(self):
        return run_list(pi, environ=self.environ, home=self.home)

    def test_lists_pinned_then_recent_projects(self):
        older, newer, pinned = self.project("older"), self.project("newer"), self.project("pinned")
        self.session(older, 100, "a")
        self.session(newer, 200, "b")
        self.session(str(self.home), 300, "c")
        pi.main(["pin", pinned], environ=self.environ)

        rows = self.rows()

        self.assertEqual([row["label"] for row in rows], ["pinned", "newer", "older"])
        self.assertEqual(rows[0]["secondary"]["label"], "Unpin")
        self.assertEqual(rows[1]["secondary"]["action"], ["python3", "modes/pi.py", "pin", newer])
        self.assertEqual(rows[1]["detail"], "~/newer · last used")
        self.assertEqual(rows[1]["action"],
                         ["setsid", "uwsm-app", "--", "xdg-terminal-exec", f"--dir={newer}", "pi"])

    def test_pinned_recent_project_is_listed_once(self):
        path = self.project("both")
        self.session(path, 100)
        pi.main(["pin", path], environ=self.environ)
        self.assertEqual([row["label"] for row in self.rows()], ["both"])

    def test_missing_pinned_folder_is_disabled_but_unpinnable(self):
        pi.main(["pin", str(self.home / "gone")], environ=self.environ)
        [row] = self.rows()
        self.assertTrue(row["disabled"])
        self.assertEqual(row["detail"], "~/gone · missing")
        self.assertEqual(row["secondary"]["label"], "Unpin")

    def test_unpin_removes_and_pin_does_not_duplicate(self):
        path = self.project("project")
        pi.main(["pin", path], environ=self.environ)
        pi.main(["pin", path], environ=self.environ)
        self.assertEqual(pi.read_pinned(self.environ), [path])
        pi.main(["unpin", path], environ=self.environ)
        self.assertEqual(pi.read_pinned(self.environ), [])

    def test_reads_pins_from_the_old_bar_widget_until_changed(self):
        kept, added = self.project("kept"), self.project("added")
        legacy = pi.legacy_pinned_file(self.environ)
        legacy.parent.mkdir(parents=True)
        legacy.write_text(json.dumps({"projects": [kept]}))

        self.assertEqual(pi.read_pinned(self.environ), [kept])
        pi.main(["pin", added], environ=self.environ)

        self.assertEqual(json.loads(pi.pinned_file(self.environ).read_text())["projects"], [kept, added])

    def test_invalid_pin_file_lists_no_pins(self):
        file = pi.pinned_file(self.environ)
        file.parent.mkdir(parents=True)
        file.write_text("{")
        with mock.patch("sys.stderr", new=io.StringIO()):
            self.assertEqual(pi.read_pinned(self.environ), [])

    def test_relative_session_dir_falls_back_to_default(self):
        agent = self.root / "agent"
        agent.mkdir()
        (agent / "settings.json").write_text(json.dumps({"sessionDir": "rel"}))
        with mock.patch("sys.stderr", new=io.StringIO()) as err:
            folder = pi.sessions_dir({"PI_CODING_AGENT_DIR": str(agent)}, self.home)
        self.assertEqual(folder, agent / "sessions")
        self.assertIn("relative sessionDir", err.getvalue())

    def test_unreadable_newest_session_falls_back_to_older(self):
        path = self.project("project")
        self.session(path, 100)
        broken = self.sessions / "group" / "200.jsonl"
        broken.write_text("not json\n")
        os.utime(broken, (200, 200))
        self.assertEqual([row["label"] for row in self.rows()], ["project"])


if __name__ == "__main__":
    unittest.main()
