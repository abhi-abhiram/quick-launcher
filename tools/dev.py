#!/usr/bin/env python3
"""Run the v1 core from a terminal, as a minimal frontend.

Builds the workspace, lays out each bundled plugin as target/plugins/<id>/
and starts the core with `--bundled-plugins target/plugins`. It uses the
real platform, so activating an app really launches it.

    > fire              type a line to send it as input
     1. Firefox         Web Browser
    > !1                run item 1's default action (!!1: its alternate)

`<` sends leave_mode, `\\t` (or a tab) sends complete, and an empty line
clears the text. Ctrl+D quits.
"""

import argparse
import subprocess
import sys
import threading
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from core_client import VERSION, CoreClient  # noqa: E402
from plugin_layout import lay_out  # noqa: E402

try:
    import readline  # noqa: F401  (line editing for input())
except ImportError:
    pass

ROOT = Path(__file__).resolve().parent.parent
PROMPT = "> "


def build(release):
    argv = ["cargo", "build", "--workspace", "--bins"] + (["--release"] if release else [])
    subprocess.run(argv, cwd=ROOT, check=True)
    profile = "release" if release else "debug"
    return ROOT / "target" / profile


def show(state):
    lines = []
    if state["mode"]:
        lines.append(f"[{state['mode']['name']}]")
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
    print("\n" + "\n".join(lines), flush=True)
    print(PROMPT, end="", flush=True)


def print_messages(client):
    while True:
        message = client.recv(timeout=None)
        if message is None:
            print("\nthe core exited", flush=True)
            return
        if message["type"] == "state":
            show(message)
        elif message["type"] == "done":
            print("\ndone; opening a new session", flush=True)
            client.open()
        elif message["type"] == "ready" and message["v"] != VERSION:
            print(f"\nthe core speaks frontend protocol {message['v']}, not {VERSION}", flush=True)
            return


def item_action(client, command):
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

    bins = build(args.release)
    plugins = ROOT / "target" / "plugins"
    lay_out(plugins, bins)
    client = CoreClient([str(bins / "quick-launcher-core"), "--bundled-plugins", str(plugins)])
    client.hello()
    client.open()
    threading.Thread(target=print_messages, args=(client,), daemon=True).start()

    try:
        while True:
            line = input(PROMPT)
            if line in ("\\t", "\t"):
                client.send("complete")
            elif line == "<":
                client.send("leave_mode")
            elif line.startswith("!") and len(line) > 1:
                target = item_action(client, line)
                if target:
                    client.activate(*target)
            else:
                client.input(line)
    except (EOFError, KeyboardInterrupt):
        print()
    except BrokenPipeError:
        pass
    return client.finish()


if __name__ == "__main__":
    sys.exit(main())
