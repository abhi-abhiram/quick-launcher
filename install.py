#!/usr/bin/env python3
"""Install the quick launcher as an Omarchy shell plugin."""

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

# Keys are the user's to choose, so the installer only suggests a binding.
TOGGLE_COMMAND = f"omarchy-shell shell toggle {PLUGIN_ID} '{{}}'"
BINDING_HINT = f"""Bind a key in ~/.config/hypr/bindings.lua, for example:
  hl.unbind("SUPER + SPACE")
  o.bind("SUPER + SPACE", "Quick launcher", "{TOGGLE_COMMAND}")"""


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


def is_bound(config_home: Path) -> bool:
    try:
        return PLUGIN_ID in (config_home / "hypr/bindings.lua").read_text()
    except OSError:
        return False


def reload_desktop() -> None:
    """Ask the running shell to pick up the plugin changes."""
    command = ["omarchy-shell", "shell", "rescanPlugins"]
    try:
        subprocess.run(command, capture_output=True, timeout=10, check=False)
    except (OSError, subprocess.SubprocessError) as error:
        print(f"Could not run {' '.join(command)}: {error}")


def install(config_home: Path) -> list:
    messages = []
    replaced = install_files(config_home)
    messages.append(f"Installed {PLUGIN_ID} in {plugin_dir(config_home)}")
    saved = update_shell_config(config_home, enable=True)
    if saved:
        messages.append(f"Enabled the plugin in shell.json (backup: {saved})")
    for path in remove_legacy_widget_files(config_home):
        messages.append(f"Removed the old π bar button: {path}")
    if not is_bound(config_home):
        messages.append(BINDING_HINT)
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
    if is_bound(config_home):
        messages.append(f"Remove the {PLUGIN_ID} binding from ~/.config/hypr/bindings.lua.")
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
                        help="Don't ask the running shell to reload plugins")
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
