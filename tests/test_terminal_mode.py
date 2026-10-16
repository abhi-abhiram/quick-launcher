import importlib.util
import io
import json
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


terminal = load("terminal")


def run_list(module, **kwargs):
    with mock.patch("sys.stdout", new=io.StringIO()) as out:
        module.main(["list"], **kwargs)
    return json.loads(out.getvalue())["rows"]


class TerminalModeTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.environ = {"XDG_STATE_HOME": self.temp.name}

    def tearDown(self):
        self.temp.cleanup()

    def test_history_is_most_recent_first_without_duplicates(self):
        for command in ["ls", "htop", "ls"]:
            terminal.remember(command, self.environ)
        rows = run_list(terminal, environ=self.environ)
        self.assertEqual([row["label"] for row in rows], ["ls", "htop"])
        self.assertEqual(rows[0]["action"], ["python3", "modes/terminal.py", "run", "ls"])
        self.assertEqual(rows[0]["secondary"]["action"], ["python3", "modes/terminal.py", "forget", "ls"])

    def test_forget_removes_a_command(self):
        terminal.remember("ls", self.environ)
        terminal.main(["forget", "ls"], environ=self.environ)
        self.assertEqual(terminal.read_history(self.environ), [])

    def test_history_is_capped(self):
        for index in range(terminal.HISTORY_LIMIT + 5):
            terminal.remember(f"echo {index}", self.environ)
        history = terminal.read_history(self.environ)
        self.assertEqual(len(history), terminal.HISTORY_LIMIT)
        self.assertEqual(history[0], f"echo {terminal.HISTORY_LIMIT + 4}")

    def test_run_remembers_then_opens_a_terminal_that_stays_open(self):
        with mock.patch.object(terminal, "working_directory", return_value="/work"), \
             mock.patch("os.execvp") as execvp:
            terminal.main(["run", "make test"], environ=self.environ)
        self.assertEqual(terminal.read_history(self.environ), ["make test"])
        execvp.assert_called_once_with("setsid", [
            "setsid", "uwsm-app", "--", "xdg-terminal-exec", "--dir=/work",
            "bash", "-ic", 'eval "$1"; exec "${SHELL:-bash}"', "bash", "make test",
        ])

    def test_blank_command_is_rejected(self):
        with self.assertRaises(SystemExit):
            terminal.main(["run", "  "], environ=self.environ)


if __name__ == "__main__":
    unittest.main()
