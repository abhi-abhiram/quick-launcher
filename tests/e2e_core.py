#!/usr/bin/env python3
"""End-to-end test of quick-launcher-core over the frontend protocol.

Builds the core with the `recording-platform` feature (in its own target
directory, so the normal build is untouched) and the fixture plugins, then
drives the binary with `tools/core_client.py`, the client `tools/dev.py`
uses. Plugins live in temporary XDG directories and every platform call is
written to a file instead of run, so nothing launches and no Omarchy,
Hyprland or Quickshell is needed.

Run it with `python3 tests/e2e_core.py`. It isn't named test_*.py, so v0's
`unittest discover` doesn't pick it up without a Rust toolchain.
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tools"))

from core_client import CoreClient  # noqa: E402

TARGET = ROOT / "target" / "e2e"
BINS = TARGET / "debug"


def setUpModule():
    for package, extra in [
        ("quick-launcher-core", ["--features", "recording-platform"]),
        ("quick-launcher-core-fixtures", []),
    ]:
        subprocess.run(
            ["cargo", "build", "--bins", "-p", package, "--target-dir", str(TARGET), *extra],
            cwd=ROOT,
            check=True,
        )


class Core(unittest.TestCase):
    """Each test gets a temporary home and starts the core there."""

    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.home = Path(temp.name)
        self.bundled = self.home / "data/quick-launcher/plugins"
        self.record = self.home / "calls.jsonl"
        self.log = self.home / "core.log"
        self.client = None

    def add_plugin(self, plugin_id, fixture, extra="", where=None):
        folder = (where or self.bundled) / plugin_id
        folder.mkdir(parents=True)
        (folder / "plugin.toml").write_text(
            "protocol = 1\n"
            f'id = "{plugin_id}"\n'
            f'name = "{plugin_id.capitalize()}"\n'
            f'prefixes = ["{plugin_id}:"]\n'
            'bin = "bin"\n'
            f"{extra}\n"
        )
        (folder / "bin").symlink_to(BINS / f"fixture-{fixture}")

    def start(self, *args):
        env = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": str(self.home),
            "XDG_DATA_HOME": str(self.home / "data"),
            "XDG_CONFIG_HOME": str(self.home / "config"),
            "XDG_STATE_HOME": str(self.home / "state"),
            "QL_RECORD": str(self.record),
        }
        log = self.log.open("w")
        self.addCleanup(log.close)
        self.client = CoreClient([str(BINS / "quick-launcher-core"), *args], env=env, stderr=log)
        self.addCleanup(self.client.finish)
        return self.client

    def ready(self, **hello):
        client = self.start()
        client.hello(**hello)
        message = client.recv()
        self.assertEqual(message["type"], "ready")
        self.assertEqual(message["v"], 1)
        return client

    def calls(self):
        if not self.record.exists():
            return []
        return [json.loads(line) for line in self.record.read_text().splitlines()]

    def log_text(self):
        return self.log.read_text()


def refs(state):
    return [item["ref"] for item in state["items"]]


class PlainSearch(Core):
    def setUp(self):
        super().setUp()
        self.add_plugin("alpha", "list", 'global = true\nicon = "α"')
        self.add_plugin("beta", "list", "global = true")
        self.add_plugin("hidden", "list")

    def test_hello_open_input_activate_close(self):
        client = self.ready()
        client.open()
        state = client.settled_state()
        self.assertEqual(refs(state), ["alpha/one", "alpha/two", "beta/one", "beta/two"])
        self.assertIsNone(state["error"])
        self.assertIsNone(state["mode"])
        first = state["items"][0]
        self.assertEqual(
            first,
            {
                "ref": "alpha/one",
                "label": "One",
                "icon": {"glyph": "α"},
                "disabled": False,
                "actions": [{"id": "copy", "title": "Copy"}],
            },
        )

        client.input("tw")
        state = client.wait_for(lambda m: m["type"] == "state")
        self.assertEqual(state["seq"], 1)
        self.assertEqual(state["text"], "tw")
        self.assertEqual(refs(state), ["alpha/two", "beta/two"])

        client.activate("beta/two", "copy")
        self.assertEqual(client.recv()["type"], "done")
        self.assertEqual(self.calls(), [{"copy": "two"}])

        client.open()
        state = client.settled_state()
        self.assertEqual(state["text"], "")
        self.assertEqual(len(state["items"]), 4)
        client.send("close")
        self.assertEqual(client.finish(), 0)
        self.assertEqual(self.calls(), [{"copy": "two"}], "close runs nothing")
        self.assertIn("alpha: listing two items", self.log_text())

    def test_max_items_caps_the_state(self):
        client = self.ready(max_items=3)
        client.open()
        self.assertEqual(refs(client.settled_state()), ["alpha/one", "alpha/two", "beta/one"])

    def test_an_unknown_ref_runs_nothing_and_resends_the_state(self):
        client = self.ready()
        client.open()
        client.settled_state()
        client.activate("alpha/gone", "copy")
        state = client.wait_for(lambda m: m["type"] in ("state", "done"))
        self.assertEqual(state["type"], "state")
        self.assertEqual(len(state["items"]), 4)
        self.assertEqual(self.calls(), [])

    def test_unknown_messages_and_fields_are_ignored(self):
        client = self.ready()
        client.send("resize", width=80)
        client.send("open", theme="dark")
        state = client.settled_state()
        self.assertEqual(len(state["items"]), 4)
        self.assertIn('unknown type "resize"', self.log_text())

    def test_eof_on_stdin_exits_zero(self):
        client = self.ready()
        self.assertEqual(client.finish(), 0)


class Versions(Core):
    def test_a_version_mismatch_gets_ready_then_eof_and_a_failure(self):
        client = self.start()
        client.hello(v=2)
        message = client.recv()
        self.assertEqual(message["type"], "ready")
        self.assertEqual(message["v"], 1)
        self.assertIsNone(client.recv(), "the core closes stdout")
        self.assertNotEqual(client.process.wait(10), 0)
        self.assertIn("speaks frontend protocol 2", self.log_text())


    def test_an_invalid_hello_gets_ready_then_eof_and_a_failure(self):
        client = self.start()
        client.write({"type": "hello"})
        self.assertEqual(client.recv()["type"], "ready")
        self.assertIsNone(client.recv())
        self.assertNotEqual(client.process.wait(10), 0)


class Help(unittest.TestCase):
    def test_help_prints_usage_and_exits_zero(self):
        result = subprocess.run(
            [str(BINS / "quick-launcher-core"), "--help"], capture_output=True, text=True, timeout=10
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("usage:", result.stdout)


class Shutdown(Core):
    def test_eof_with_a_request_outstanding_exits_promptly_and_stops_the_plugin(self):
        self.add_plugin("slow", "slow", "global = true\ntimeout_ms = 600000")
        client = self.ready()
        client.open()
        self.assertTrue(client.wait_for(lambda m: m["type"] == "state")["loading"])
        pid_file = self.home / "state/quick-launcher/slow/pid"
        deadline = time.monotonic() + 10
        while not pid_file.exists():
            self.assertLess(time.monotonic(), deadline, "the slow plugin never started")
            time.sleep(0.01)
        pid = int(pid_file.read_text())
        started = time.monotonic()
        self.assertEqual(client.finish(timeout=5), 0)
        self.assertLess(time.monotonic() - started, 3)
        deadline = time.monotonic() + 5
        while running(pid):
            self.assertLess(time.monotonic(), deadline, f"plugin process {pid} is still running")
            time.sleep(0.01)


def running(pid):
    """Whether `pid` is a live process (not gone, and not a zombie)."""
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
    except FileNotFoundError:
        return False
    return stat.rsplit(")", 1)[1].split()[0] != "Z"


class Errors(Core):
    def test_a_broken_plugin_is_an_error_beside_the_other_items(self):
        self.add_plugin("alpha", "list", "global = true")
        self.add_plugin("broken", "garbage", "global = true")
        client = self.ready()
        client.open()
        state = client.settled_state()
        self.assertEqual(refs(state), ["alpha/one", "alpha/two"])
        self.assertEqual(state["error"], "Broken sent an answer the launcher can't read")


class BundledPluginsFlag(Core):
    def test_the_flag_replaces_the_bundled_directory(self):
        elsewhere = self.home / "elsewhere"
        self.add_plugin("alpha", "list", "global = true")
        self.add_plugin("gamma", "list", "global = true", where=elsewhere)
        client = self.start("--bundled-plugins", str(elsewhere))
        client.hello()
        client.recv()
        client.open()
        self.assertEqual(refs(client.settled_state()), ["gamma/one", "gamma/two"])


class Rescan(Core):
    def test_open_picks_up_an_added_plugin(self):
        self.add_plugin("alpha", "list", "global = true")
        client = self.ready()
        client.open()
        self.assertEqual(len(client.settled_state()["items"]), 2)
        self.add_plugin("beta", "list", "global = true")
        client.open()
        self.assertEqual(len(client.settled_state()["items"]), 4)


if __name__ == "__main__":
    unittest.main()
