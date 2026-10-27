#!/usr/bin/env python3
"""Run the v1 core from a terminal, as a minimal frontend.

Builds the workspace, lays out each bundled plugin as target/plugins/<id>/
(plus the SDK's `repos` example, as protocol.md's *Examples* installs it)
and starts the core with `--bundled-plugins target/plugins`. It uses the
real platform, so activating an app really launches it.

    > fire              type a line to send it as input
     1. Firefox         Web Browser
    > !1                run item 1's default action (!!1: its alternate)
    > repo:             a prefix enters a mode; the prompt shows it
    Repos> <            leave the mode

Each line is the whole text typed in the prompt's mode. `<` sends
leave_mode, `\\t` (or a tab) sends complete, and an empty line clears the
text. Ctrl+D quits.
"""

import argparse
import subprocess
import sys
import threading
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from core_client import VERSION, CoreClient  # noqa: E402
from plugin_layout import lay_out, lay_out_example  # noqa: E402

try:
    import readline  # line editing for input()
except ImportError:
    readline = None

ROOT = Path(__file__).resolve().parent.parent
EXAMPLES = ROOT / "crates" / "plugin" / "examples"

client = None
# Set while input() waits for a line, so a state that arrives meanwhile
# redraws the prompt below it.
reading = threading.Event()
# Set when a message arrives, so the next prompt waits for the answer to the
# last command instead of being printed above it.
answered = threading.Event()


def build(release):
    argv = ["cargo", "build", "--workspace", "--bins", "--examples"] + (["--release"] if release else [])
    subprocess.run(argv, cwd=ROOT, check=True)
    profile = "release" if release else "debug"
    return ROOT / "target" / profile


def prompt():
    """`> ` in the plain search, `<mode name>> ` in a mode."""
    if client.mode is None:
        return "> "
    mode = (client.state or {}).get("mode") or {}
    return f"{mode.get('name') if mode.get('id') == client.mode else client.mode}> "


def show(state):
    lines = []
    if state["error"]:
        lines.append(f"  ! {state['error']}")
    width = max((len(item["label"]) for item in state["items"]), default=0)
    width = min(width, 32)
    for n, item in enumerate(state["items"], 1):
        label = item["label"]
        if len(label) > width:
            label = label[: width - 1] + "…"
        disabled = " (disabled)" if item.get("disabled") else ""
        lines.append(f"{n:>2}. {label:<{width}}  {item.get('detail', '')}{disabled}".rstrip())
    if state["loading"]:
        lines.append("  … loading")
    if not state["items"] and not state["loading"]:
        lines.append("  (no items)")
    text = "\n".join(lines)
    if reading.is_set():
        # input() is waiting: clear its prompt line, print the state, then
        # draw the prompt again with what was typed so far.
        typed = readline.get_line_buffer() if readline else ""
        sys.stdout.write(f"\r\033[K{text}\n{prompt()}{typed}")
        sys.stdout.flush()
    else:
        print(text, flush=True)


def print_messages():
    while True:
        message = client.recv(timeout=None)
        if message is None:
            print("\nthe core exited", flush=True)
            answered.set()
            return
        if message["type"] == "state":
            show(message)
        elif message["type"] == "done":
            print("done; opening a new session", flush=True)
            client.open()
            continue  # the prompt waits for the new session's state
        elif message["type"] == "ready" and message["v"] != VERSION:
            print(f"\nthe core speaks frontend protocol {message['v']}, not {VERSION}", flush=True)
            answered.set()
            return
        answered.set()


def send(command):
    """Sends one line typed at the prompt."""
    if command in ("\\t", "\t"):
        client.send("complete")
    elif command == "<":
        client.send("leave_mode")
    elif command.startswith("!") and len(command) > 1:
        target = item_action(command)
        if not target:
            return False
        client.activate(*target)
    else:
        client.input(command)
    return True


def item_action(command):
    alternate = command.startswith("!!")
    number = command[2:] if alternate else command[1:]
    try:
        item = client.state["items"][int(number) - 1]
    except (TypeError, ValueError, IndexError):
        print(f"no item {number!r}")
        return None
    actions = item["actions"]
    index = 1 if alternate else 0
    if index >= len(actions):
        print(f"{item['label']} has no alternate action")
        return None
    return item["ref"], actions[index]["id"]


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--release", action="store_true", help="build and run release binaries")
    args = parser.parse_args()

    global client
    bins = build(args.release)
    plugins = ROOT / "target" / "plugins"
    lay_out(plugins, bins)
    lay_out_example(plugins, EXAMPLES / "repos.plugin.toml", bins / "examples" / "repos")
    client = CoreClient([str(bins / "quick-launcher-core"), "--bundled-plugins", str(plugins)])
    client.hello()
    client.open()
    threading.Thread(target=print_messages, daemon=True).start()

    try:
        while True:
            # A state usually answers within milliseconds; print it before
            # the prompt rather than through it.
            answered.wait(timeout=1)
            reading.set()
            try:
                line = input(prompt())
            finally:
                reading.clear()
            answered.clear()
            if not send(line):
                answered.set()
    except (EOFError, KeyboardInterrupt):
        print()
    except BrokenPipeError:
        pass
    return client.finish()


if __name__ == "__main__":
    sys.exit(main())
