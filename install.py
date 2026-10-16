#!/usr/bin/env python3
"""Install the quick launcher as an Omarchy shell plugin on SUPER+ALT+SPACE."""

import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


PLUGIN_ID = "abhiram.quick-launcher"
PLUGIN_FILES = (
    "manifest.json", "Launcher.qml", "LauncherModel.js", "modes.json",
    "modes/pi.py", "modes/terminal.py",
)
# The pi-launcher bar button this launcher replaces.
LEGACY_WIDGET_ID = "pi-launcher"
LEGACY_WIDGET_FILES = ("pi-launcher.qml", "pi-launcher-projects.py")

BINDING_BEGIN = "-- quick-launcher:begin"
BINDING_END = "-- quick-launcher:end"
BINDING = f"""{BINDING_BEGIN} (managed by quick-launcher's install.py)
-- Replaces the Omarchy apps menu.
hl.unbind("SUPER + ALT + SPACE")
o.bind("SUPER + ALT + SPACE", "Quick launcher", "omarchy-shell shell toggle {PLUGIN_ID} '{{}}'")
{BINDING_END}
"""


def stamp() -> str:
    return f"{datetime.now():%Y%m%d-%H%M%S-%f}"


def write_atomic(path: Path, text: str) -> None:
    """Replace a watched file in one step so no reader sees it half-written."""
    fd, temporary = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}-")
    try:
        with os.fdopen(fd, "w") as stream:
            stream.write(text)
        if path.exists():
            shutil.copymode(path, temporary)
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def backup(path: Path) -> Path:
    copy = path.with_name(f"{path.name}.bak.{stamp()}")
    shutil.copy2(path, copy)
    return copy


def plugin_dir(config_home: Path) -> Path:
    return config_home / "omarchy/plugins" / PLUGIN_ID


def plugin_files(source: Path) -> list[str]:
    """The fixed plugin files plus every script under modes/, so modes added
    as described in the README are installed too."""
    extra = sorted(
        path.relative_to(source).as_posix()
        for path in (source / "modes").rglob("*")
        if path.is_file() and "__pycache__" not in path.parts
    )
    return list(dict.fromkeys([*PLUGIN_FILES, *extra]))


def install_files(config_home: Path) -> bool:
    """Copy the plugin into place. Returns whether an older copy was replaced."""
    source = Path(__file__).resolve().parent
    files = plugin_files(source)
    for name in files:
        if not (source / name).is_file():
            raise FileNotFoundError(f"Missing plugin file: {source / name}")
    target = plugin_dir(config_home)
    target.parent.mkdir(parents=True, exist_ok=True)
    # Stage beside the target and swap, so the shell's plugin watcher never
    # loads a half-copied plugin.
    staged = Path(tempfile.mkdtemp(dir=target.parent, prefix=f".{PLUGIN_ID}-"))
    try:
        for name in files:
            (staged / name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source / name, staged / name)
        staged.chmod(0o755)
        replaced = target.exists()
        if replaced:
            retired = target.with_name(f".{PLUGIN_ID}-old-{stamp()}")
            target.rename(retired)
            staged.rename(target)
            shutil.rmtree(retired)
        else:
            staged.rename(target)
        return replaced
    finally:
        shutil.rmtree(staged, ignore_errors=True)


def update_shell_config(config_home: Path, enable: bool):
    """Enable or disable the plugin and drop the legacy bar button.

    Returns the backup path, or None when nothing needed to change.
    """
    config = config_home / "omarchy/shell.json"
    data = json.loads(config.read_text())
    original = json.dumps(data, sort_keys=True)

    plugins = data.get("plugins", [])
    if not isinstance(plugins, list):
        raise ValueError("Expected plugins to be a list")
    plugins = [entry for entry in plugins
               if not (isinstance(entry, dict) and entry.get("id") == PLUGIN_ID)]
    if enable:
        plugins.append({"id": PLUGIN_ID})
        layout = data.get("bar", {}).get("layout", {})
        if isinstance(layout, dict):
            for section, entries in layout.items():
                if isinstance(entries, list):
                    layout[section] = [entry for entry in entries if not (
                        isinstance(entry, dict) and entry.get("id") == LEGACY_WIDGET_ID)]
    data["plugins"] = plugins

    if json.dumps(data, sort_keys=True) == original:
        return None
    saved = backup(config)
    write_atomic(config, json.dumps(data, indent=2, ensure_ascii=False) + "\n")
    return saved


def remove_legacy_widget_files(config_home: Path) -> list:
    removed = []
    for name in LEGACY_WIDGET_FILES:
        path = config_home / "omarchy/bar/modules" / name
        if path.is_file():
            path.unlink()
            removed.append(path)
    return removed


def without_binding(text: str) -> str:
    if BINDING_BEGIN not in text:
        return text
    lines = text.splitlines(keepends=True)
    kept, inside = [], False
    for line in lines:
        if line.startswith(BINDING_BEGIN):
            inside = True
        elif inside and line.startswith(BINDING_END):
            inside = False
        elif not inside:
            kept.append(line)
    return "".join(kept).rstrip("\n") + "\n" if kept else ""


def update_bindings(config_home: Path, add: bool):
    """Add or remove the managed keybinding block. Returns the backup path."""
    bindings = config_home / "hypr/bindings.lua"
    current = bindings.read_text() if bindings.exists() else ""
    updated = without_binding(current)
    if add:
        updated += ("\n" if updated else "") + BINDING
    if updated == current:
        return None
    saved = backup(bindings) if bindings.exists() else None
    bindings.parent.mkdir(parents=True, exist_ok=True)
    write_atomic(bindings, updated)
    return saved


def reload_desktop() -> None:
    """Ask the running shell and Hyprland to pick up the changes."""
    commands = [["omarchy-shell", "shell", "rescanPlugins"], ["hyprctl", "reload"]]
    for command in commands:
        try:
            subprocess.run(command, capture_output=True, timeout=10, check=False)
        except (OSError, subprocess.SubprocessError) as error:
            print(f"Could not run {' '.join(command)}: {error}")
    try:
        errors = subprocess.run(["hyprctl", "configerrors"], capture_output=True,
                                text=True, timeout=10).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return
    if errors:
        print(f"Hyprland reports config errors:\n{errors}")


def install(config_home: Path) -> list:
    messages = []
    replaced = install_files(config_home)
    messages.append(f"Installed {PLUGIN_ID} in {plugin_dir(config_home)}")
    saved = update_shell_config(config_home, enable=True)
    if saved:
        messages.append(f"Enabled the plugin in shell.json (backup: {saved})")
    for path in remove_legacy_widget_files(config_home):
        messages.append(f"Removed the old π bar button: {path}")
    saved = update_bindings(config_home, add=True)
    if saved:
        messages.append(f"Bound SUPER+ALT+SPACE (was: Omarchy apps menu; backup: {saved})")
    if replaced:
        # keepLoaded plugins stay mounted across hot reloads.
        messages.append("Updated an existing install: run `omarchy restart shell` to load it.")
    return messages


def uninstall(config_home: Path) -> list:
    messages = []
    target = plugin_dir(config_home)
    if target.exists():
        shutil.rmtree(target)
        messages.append(f"Removed {target}")
    saved = update_shell_config(config_home, enable=False)
    if saved:
        messages.append(f"Disabled the plugin in shell.json (backup: {saved})")
    saved = update_bindings(config_home, add=False)
    if saved:
        messages.append(f"SUPER+ALT+SPACE opens the Omarchy apps menu again (backup: {saved})")
    return messages


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--config-home", type=Path,
        default=Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")),
        help="Configuration directory (default: $XDG_CONFIG_HOME or ~/.config)",
    )
    parser.add_argument("--uninstall", action="store_true", help="Remove the launcher")
    parser.add_argument("--no-reload", action="store_true",
                        help="Don't ask the running shell and Hyprland to reload")
    args = parser.parse_args()
    try:
        messages = uninstall(args.config_home) if args.uninstall else install(args.config_home)
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f"{'Uninstall' if args.uninstall else 'Install'} failed: {error}\n")
    print("\n".join(messages))
    if not args.no_reload:
        reload_desktop()


if __name__ == "__main__":
    main()
