#!/usr/bin/env python3
"""terminal mode: run shell commands in a new terminal and remember them.

Usage:
  terminal.py list            Print previously run commands as launcher rows
  terminal.py run COMMAND     Run COMMAND in a new terminal and remember it
  terminal.py forget COMMAND  Remove COMMAND from the history
"""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


HISTORY_LIMIT = 100


def history_file(environ=os.environ) -> Path:
    state = Path(environ.get("XDG_STATE_HOME") or Path.home() / ".local/state")
    return state / "quick-launcher/terminal-history.json"


def read_history(environ=os.environ) -> list:
    """Return remembered commands, most recent first."""
    try:
        commands = json.loads(history_file(environ).read_text()).get("commands")
    except FileNotFoundError:
        return []
    except (OSError, ValueError, AttributeError) as error:
        print(f"quick-launcher: ignoring invalid terminal history: {error}", file=sys.stderr)
        return []
    if not isinstance(commands, list):
        return []
    return [command for command in commands if isinstance(command, str) and command.strip()]


def write_history(commands: list, environ=os.environ) -> None:
    file = history_file(environ)
    file.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(dir=file.parent, prefix=".terminal-history-", suffix=".json")
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump({"commands": commands[:HISTORY_LIMIT]}, stream, indent=2, ensure_ascii=False)
            stream.write("\n")
        os.replace(temporary, file)
    finally:
        Path(temporary).unlink(missing_ok=True)


def remember(command: str, environ=os.environ) -> None:
    write_history([command] + [item for item in read_history(environ) if item != command], environ)


def forget(command: str, environ=os.environ) -> None:
    write_history([item for item in read_history(environ) if item != command], environ)


def rows(commands: list) -> list:
    return [{
        "id": command,
        "label": command,
        "detail": "Run again",
        "icon": "",
        "action": ["python3", "modes/terminal.py", "run", command],
        "secondary": {"label": "Forget", "action": ["python3", "modes/terminal.py", "forget", command]},
    } for command in commands]


def working_directory(home: Path = Path.home()) -> str:
    """Use the focused terminal's folder, like Omarchy's new-terminal binding."""
    try:
        result = subprocess.run(["omarchy-cmd-terminal-cwd"], capture_output=True, text=True, timeout=2)
        cwd = result.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        cwd = ""
    return cwd if cwd and os.path.isdir(cwd) else str(home)


def terminal_command(command: str, cwd: str) -> list:
    # An interactive bash expands the user's aliases, then hands over to their
    # shell so the output stays readable after the command exits.
    return ["setsid", "uwsm-app", "--", "xdg-terminal-exec", f"--dir={cwd}",
            "bash", "-ic", 'eval "$1"; exec "${SHELL:-bash}"', "bash", command]


def main(argv=None, environ=os.environ):
    argv = sys.argv[1:] if argv is None else argv
    command, args = (argv[0], argv[1:]) if argv else ("list", [])
    if command == "list" and not args:
        json.dump({"rows": rows(read_history(environ))}, sys.stdout, ensure_ascii=False)
        sys.stdout.write("\n")
    elif command == "run" and len(args) == 1 and args[0].strip():
        remember(args[0], environ)
        os.execvp("setsid", terminal_command(args[0], working_directory()))
    elif command == "forget" and len(args) == 1:
        forget(args[0], environ)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
