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
from plugin_layout import lay_out  # noqa: E402

TARGET = ROOT / "target" / "e2e"
BINS = TARGET / "debug"


def setUpModule():
    for package, extra in [
        ("quick-launcher-core", ["--features", "recording-platform"]),
        ("quick-launcher-core-fixtures", []),
        # The bundled plugins, which `BundledPlugins` lays out.
        ("quick-launcher-apps", []),
        ("quick-launcher-pi", []),
        ("quick-launcher-terminal", []),
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
            "PATH": os.pathsep.join([str(self.home / "bin"), os.environ.get("PATH", "")]),
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


class Modes(Core):
    def setUp(self):
        super().setUp()
        self.add_plugin("alpha", "list", "global = true")
        self.add_plugin("hidden", "list")
        self.add_plugin("stream", "stream", 'kind = "query"')

    def state_for(self, client, seq):
        """The settled state answering the `input` with this seq."""
        return client.wait_for(lambda m: m["type"] == "state" and m["seq"] >= seq and not m["loading"])

    def test_a_prefix_enters_a_list_mode_and_leave_mode_leaves_it(self):
        client = self.ready()
        client.open()
        client.settled_state()

        client.input("hidden:")
        state = self.state_for(client, 1)
        self.assertEqual(state["mode"], {"id": "hidden", "name": "Hidden"})
        self.assertEqual(state["text"], "")
        self.assertEqual(refs(state), ["hidden/one", "hidden/two"])
        self.assertEqual(client.mode, "hidden")

        client.input("tw")  # read in the mode
        state = self.state_for(client, 2)
        self.assertEqual(state["mode"]["id"], "hidden")
        self.assertEqual(refs(state), ["hidden/two"])

        client.input("")
        self.state_for(client, 3)
        client.send("leave_mode")
        state = client.wait_for(lambda m: m["type"] == "state")
        self.assertIsNone(state["mode"])
        self.assertEqual(state["text"], "hidden")
        self.assertEqual(state["seq"], 3)
        self.assertIsNone(client.mode)

        client.input("hidden:")
        self.state_for(client, 4)
        client.activate("hidden/one", "copy")
        self.assertEqual(client.wait_for(lambda m: m["type"] == "done")["type"], "done")
        self.assertEqual(self.calls(), [{"copy": "one"}])

    def test_typing_faster_than_the_core_reads_the_prefix_once(self):
        client = self.ready()
        client.open()
        client.settled_state()
        client.input("hidden:")
        client.input("hidden:t")  # still mode null: no state was read yet
        state = self.state_for(client, 2)
        self.assertEqual(state["mode"]["id"], "hidden")
        self.assertEqual(state["text"], "t")
        self.assertEqual(refs(state), ["hidden/two"])

    def test_open_straight_into_a_mode(self):
        client = self.ready()
        client.open("hidden")
        state = client.settled_state()
        self.assertEqual(state["mode"]["id"], "hidden")
        self.assertEqual(refs(state), ["hidden/one", "hidden/two"])
        client.send("leave_mode")
        # alpha's answer may still come first, with a state in the mode.
        state = client.wait_for(lambda m: m["type"] == "state" and m["mode"] is None)
        self.assertEqual(state["text"], "hidden")

    def test_mode_hints_complete_and_enter_their_mode(self):
        client = self.ready()
        client.open()
        client.settled_state()
        client.input("hid")
        state = self.state_for(client, 1)
        hints = [item for item in state["items"] if item["label"] == "Hidden"]
        self.assertEqual(len(hints), 1)
        self.assertEqual(hints[0]["detail"], "hidden:")
        self.assertEqual(hints[0]["actions"], [{"id": "enter", "title": "Open Hidden"}])

        client.send("complete")
        state = client.wait_for(lambda m: m["type"] == "state" and m["mode"] and not m["loading"])
        self.assertEqual((state["mode"]["id"], state["text"], state["seq"]), ("hidden", "", 1))
        self.assertEqual(refs(state), ["hidden/one", "hidden/two"])

        client.send("leave_mode")
        state = client.wait_for(lambda m: m["type"] == "state" and m["mode"] is None)
        self.assertEqual(state["text"], "hidden")
        # Activating the hint enters the mode too, with a state and no done.
        client.activate(hints[0]["ref"], "enter")
        state = client.wait_for(lambda m: m["type"] == "state" and m["mode"] and not m["loading"])
        self.assertEqual(state["mode"]["id"], "hidden")
        self.assertEqual(self.calls(), [])

    def test_a_query_mode_asks_on_every_input(self):
        client = self.ready()
        client.open()
        client.settled_state()
        client.input("stream: 1")
        state = self.state_for(client, 1)
        self.assertEqual(state["mode"]["id"], "stream")
        self.assertEqual(state["text"], "1")
        self.assertEqual(refs(state), ["stream/1", "stream/2"])
        client.input("0")
        self.assertEqual(refs(self.state_for(client, 2)), ["stream/1"])


class Activation(Core):
    def setUp(self):
        super().setUp()
        self.add_plugin(
            "pins",
            "pins",
            "global = true\n"
            "[query_item]\n"
            'label = "Add {query}"\n'
            'actions = [{ id = "add", title = "Add", run = { plugin = "add" } }]',
        )

    def settled(self, client, predicate):
        return client.wait_for(lambda m: m["type"] == "state" and not m["loading"] and predicate(m))

    def test_a_plugin_action_with_refresh_shows_the_change(self):
        client = self.ready()
        client.open()
        state = client.settled_state()
        self.assertEqual(refs(state), ["pins/one", "pins/two"])
        self.assertEqual([a["id"] for a in state["items"][1]["actions"]], ["copy", "pin"])

        client.activate("pins/two", "pin")
        state = self.settled(client, lambda m: "badge" in m["items"][1])
        self.assertEqual(state["items"][1]["badge"], "pinned")
        self.assertEqual([a["id"] for a in state["items"][1]["actions"]], ["copy", "unpin"])
        self.assertIn('pins: activating "pin" on Some("two") with ""', self.log_text())
        self.assertEqual(self.calls(), [], "a plugin action runs nothing itself")

        # The launcher stayed open: the next action still runs.
        client.activate("pins/two", "unpin")
        state = self.settled(client, lambda m: "badge" not in m["items"][1])
        client.activate("pins/one", "copy")
        self.assertEqual(client.wait_for(lambda m: m["type"] == "done")["type"], "done")
        self.assertEqual(self.calls(), [{"copy": "one"}])

    def test_an_activate_error_keeps_the_items(self):
        client = self.ready()
        client.open()
        client.settled_state()
        # Something else pins `one` while the launcher still offers "pin".
        (self.home / "state/quick-launcher/pins/pinned").write_text("one")
        client.activate("pins/one", "pin")
        state = client.wait_for(lambda m: m["type"] == "state" and m["error"])
        self.assertEqual(state["error"], "Couldn't run “pin”: one is already pinned")
        self.assertEqual(refs(state), ["pins/one", "pins/two"])

    def test_a_query_item_runs_with_the_text_it_was_built_from(self):
        client = self.ready()
        client.open("pins")
        client.settled_state()
        client.input("three")
        state = self.settled(client, lambda m: m["seq"] == 1)
        self.assertEqual(refs(state)[0], "pins:query:three")
        client.input("threeX")  # newer input before the user's Enter
        client.activate("pins:query:three", "add")
        # The refresh: loading, then the plugin's new list.
        client.wait_for(lambda m: m["type"] == "state" and m["loading"])
        self.settled(client, lambda m: True)
        self.assertIn('activating "add" on None with "three"', self.log_text())
        client.input("")
        state = self.settled(client, lambda m: m["seq"] == 3)
        self.assertEqual(refs(state), ["pins/one", "pins/two", "pins/three"])


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
        # The rest is Linux only: `running` reads /proc, and elsewhere it
        # would pass without checking anything.
        if not sys.platform.startswith("linux"):
            self.skipTest("checking that the plugin is gone reads /proc")
        deadline = time.monotonic() + 5
        while running(pid):
            self.assertLess(time.monotonic(), deadline, f"plugin process {pid} is still running")
            time.sleep(0.01)


def running(pid):
    """Whether `pid` is a live process (not gone, and not a zombie). Linux
    only: it reads /proc."""
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


class BundledPlugins(Core):
    """The real bundled plugins, laid out as `tools/dev.py` lays them out."""

    def setUp(self):
        super().setUp()
        lay_out(self.bundled, BINS)
        # The recording platform's login command puts `login` in front of a
        # `login` plugin's argv. This one runs the rest as it is.
        bin_dir = self.home / "bin"
        bin_dir.mkdir()
        (bin_dir / "login").write_text('#!/bin/sh\nexec "$@"\n')
        (bin_dir / "login").chmod(0o755)

    def pi_session(self, name, mtime):
        project = self.home / "Projects" / name
        project.mkdir(parents=True)
        group = self.home / ".pi/agent/sessions" / name
        group.mkdir(parents=True)
        session = group / "session.jsonl"
        session.write_text(json.dumps({"type": "session", "cwd": str(project)}) + "\n")
        os.utime(session, (mtime, mtime))
        return str(project)

    def test_pi_lists_projects_and_pins_and_unpins_them(self):
        self.pi_session("alpha", 100)
        beta = self.pi_session("beta", 200)
        client = self.ready()
        client.open()
        client.settled_state()
        client.input("pi:")
        state = client.wait_for(
            lambda m: m["type"] == "state" and not m["loading"] and (m["mode"] or {}).get("id") == "pi"
        )
        self.assertEqual([item["label"] for item in state["items"]], ["beta", "alpha"])
        self.assertEqual(state["items"][0]["detail"], "~/Projects/beta · last used")
        self.assertEqual(state["items"][0]["icon"], {"glyph": "π"})
        self.assertEqual([a["id"] for a in state["items"][0]["actions"]], ["open", "pin"])

        client.activate(state["items"][0]["ref"], "pin")
        state = client.wait_for(
            lambda m: m["type"] == "state" and not m["loading"]
            and m["items"] and m["items"][0]["icon"] == {"glyph": "󰐃"}
        )
        self.assertEqual([a["id"] for a in state["items"][0]["actions"]], ["open", "unpin"])
        pins = self.home / "config/quick-launcher/pi/pinned.json"
        self.assertEqual(json.loads(pins.read_text()), {"projects": [beta]})

        client.activate(state["items"][0]["ref"], "unpin")
        client.wait_for(
            lambda m: m["type"] == "state" and not m["loading"]
            and m["items"] and m["items"][0]["icon"] == {"glyph": "π"}
        )
        self.assertEqual(json.loads(pins.read_text()), {"projects": []})

        client.activate(state["items"][0]["ref"], "open")
        client.wait_for(lambda m: m["type"] == "done")
        self.assertEqual(
            self.calls(),
            [{"terminal": {"argv": ["login", "pi"], "cwd": beta}}],
        )

    def terminal_state(self, client, predicate=lambda m: True):
        return client.wait_for(
            lambda m: m["type"] == "state" and not m["loading"]
            and (m["mode"] or {}).get("id") == "terminal" and predicate(m)
        )

    def test_terminal_runs_typed_commands_again_and_forgets_them(self):
        client = self.ready()
        client.open()
        client.settled_state()
        client.input("> make test")
        state = self.terminal_state(client, lambda m: m["items"])
        self.assertEqual(refs(state), ["terminal:query:make test"])
        self.assertEqual(state["items"][0]["detail"], "Run in a new terminal")

        client.activate("terminal:query:make test", "run")
        client.wait_for(lambda m: m["type"] == "done")
        script = 'eval "$1"; exec "${SHELL:-bash}"'
        run = {"terminal": {"argv": ["bash", "-ic", script, "bash", "make test"], "cwd": str(self.home)}}
        self.assertEqual(self.calls(), [run])
        history = self.home / "state/quick-launcher/terminal/history.json"
        self.assertEqual(json.loads(history.read_text()), {"commands": ["make test"]})

        client.open()
        client.settled_state()
        client.input(">")
        state = self.terminal_state(client, lambda m: m["items"])
        self.assertEqual([item["label"] for item in state["items"]], ["make test"])
        self.assertEqual([a["id"] for a in state["items"][0]["actions"]], ["run", "forget"])

        # Typing a remembered command shows it once, as the history item.
        client.input("make test")
        state = self.terminal_state(client, lambda m: m["text"] == "make test")
        self.assertEqual(refs(state), ["terminal/make test"])

        client.activate("terminal/make test", "forget")
        self.terminal_state(client, lambda m: m["text"] == "make test" and refs(m) == ["terminal:query:make test"])
        self.assertEqual(json.loads(history.read_text()), {"commands": []})
        self.assertEqual(self.calls(), [run])


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
