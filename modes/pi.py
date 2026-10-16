#!/usr/bin/env python3
"""pi mode: list pinned and recent pi projects, and pin or unpin them.

Usage:
  pi.py list           Print launcher rows as JSON
  pi.py pin FOLDER     Pin a project
  pi.py unpin FOLDER   Unpin a project
"""

import json
import os
from pathlib import Path
import sys
import tempfile


def config_home(environ=os.environ) -> Path:
    return Path(environ.get("XDG_CONFIG_HOME") or Path.home() / ".config")


def pinned_file(environ=os.environ) -> Path:
    return config_home(environ) / "quick-launcher/pi-pinned.json"


# Pins saved by the pi-launcher bar widget, read until the first change here.
def legacy_pinned_file(environ=os.environ) -> Path:
    return config_home(environ) / "pi-launcher/pinned.json"


def read_pinned(environ=os.environ) -> list:
    for file in (pinned_file(environ), legacy_pinned_file(environ)):
        try:
            projects = json.loads(file.read_text()).get("projects")
        except FileNotFoundError:
            continue
        except (OSError, ValueError, AttributeError) as error:
            print(f"quick-launcher: ignoring invalid {file}: {error}", file=sys.stderr)
            return []
        if not isinstance(projects, list):
            return []
        return [path for path in projects if isinstance(path, str) and path]
    return []


def write_pinned(paths: list, environ=os.environ) -> None:
    file = pinned_file(environ)
    file.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(dir=file.parent, prefix=".pi-pinned-", suffix=".json")
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump({"projects": paths}, stream, indent=2)
            stream.write("\n")
        os.replace(temporary, file)
    finally:
        Path(temporary).unlink(missing_ok=True)


def sessions_dir(environ=os.environ, home: Path = Path.home()) -> Path:
    """Resolve pi's session directory the same way pi does."""
    if environ.get("PI_CODING_AGENT_SESSION_DIR"):
        return Path(environ["PI_CODING_AGENT_SESSION_DIR"]).expanduser()
    agent = Path(environ.get("PI_CODING_AGENT_DIR") or home / ".pi/agent").expanduser()
    try:
        configured = json.loads((agent / "settings.json").read_text()).get("sessionDir")
    except (OSError, ValueError, AttributeError):
        configured = None
    if isinstance(configured, str):
        if Path(configured).expanduser().is_absolute():
            return Path(configured).expanduser()
        # pi resolves a relative sessionDir from each session's own working
        # directory, so there is no single folder to read recents from.
        print(f"quick-launcher: ignoring relative sessionDir {configured!r}; "
              "recent projects come from the default session folder", file=sys.stderr)
    return agent / "sessions"


def _session(files):
    """Return (mtime, cwd) from the newest session file with a readable header."""
    stamped = []
    for file in files:
        try:
            stamped.append((file.stat().st_mtime, file))
        except OSError:
            pass
    for mtime, file in sorted(stamped, reverse=True):
        try:
            with file.open() as stream:
                cwd = json.loads(stream.readline())["cwd"]
        except (OSError, ValueError, KeyError, TypeError):
            continue
        if isinstance(cwd, str):
            return mtime, cwd
    return None


def recent_projects(sessions: Path, home: Path, limit: int = 20) -> list:
    """Return existing project folders, most recently used first.

    pi groups sessions per working directory, and each session file starts with
    a header holding its `cwd`. The home and root folders are not projects.
    """
    try:
        entries = list(sessions.iterdir())
    except OSError:
        return []
    groups = [[entry] for entry in entries if entry.suffix == ".jsonl"]
    for entry in entries:
        if entry.is_dir():
            try:
                groups.append(list(entry.glob("*.jsonl")))
            except OSError:
                pass
    latest = {}
    for files in groups:
        session = _session(files)
        if not session:
            continue
        mtime, cwd = session
        if cwd in (str(home), "/"):
            continue
        if mtime > latest.get(cwd, float("-inf")) and os.path.isdir(cwd):
            latest[cwd] = mtime
    return sorted(latest, key=latest.get, reverse=True)[:limit]


def display_path(path: str, home: Path) -> str:
    home = str(home)
    return "~" + path[len(home):] if path == home or path.startswith(home + "/") else path


def project_name(path: str) -> str:
    return next((part for part in reversed(path.split("/")) if part), path)


def launch_action(path: str) -> list:
    # The folder is an argument, never shell text.
    return ["setsid", "uwsm-app", "--", "xdg-terminal-exec", f"--dir={path}", "pi"]


def rows(pinned: list, recent: list, home: Path) -> list:
    last_used = recent[0] if recent else None
    result = []
    for path, is_pinned in [(path, True) for path in pinned] + [
        (path, False) for path in recent if path not in pinned
    ]:
        missing = is_pinned and not os.path.isdir(path)
        notes = ["missing" if missing else "last used" if path == last_used else ""]
        result.append({
            "id": path,
            "label": project_name(path),
            "detail": " · ".join([display_path(path, home)] + [note for note in notes if note]),
            "icon": "󰐃" if is_pinned else "",
            "disabled": missing,
            "action": launch_action(path),
            "secondary": {
                "label": "Unpin" if is_pinned else "Pin",
                "action": ["python3", "modes/pi.py", "unpin" if is_pinned else "pin", path],
            },
        })
    return result


def main(argv=None, environ=os.environ, home: Path = Path.home()):
    argv = sys.argv[1:] if argv is None else argv
    command, args = (argv[0], argv[1:]) if argv else ("list", [])
    if command == "list" and not args:
        pinned = read_pinned(environ)
        recent = recent_projects(sessions_dir(environ, home), home)
        json.dump({"rows": rows(pinned, recent, home)}, sys.stdout, ensure_ascii=False)
        sys.stdout.write("\n")
    elif command in ("pin", "unpin") and len(args) == 1:
        pinned = [path for path in read_pinned(environ) if path != args[0]]
        write_pinned(pinned + [args[0]] if command == "pin" else pinned, environ)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
