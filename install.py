#!/usr/bin/env python3
"""Install the quick launcher: build the core and the bundled plugins, then
install them with the Omarchy shell plugin that draws the launcher."""

import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "tools"))
import plugin_layout  # noqa: E402


PLUGIN_ID = "abhiram.quick-launcher"
# The shell plugin, relative to FRONTEND.
FRONTEND = Path("frontends/omarchy")
PLUGIN_FILES = ("manifest.json", "Launcher.qml")
CORE_BIN = "quick-launcher-core"
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


def data_dir(data_home: Path) -> Path:
    """Where the core and the bundled plugins go. The core's Omarchy
    platform reads bundled plugins from `plugins/` here, and Launcher.qml
    starts the core from `bin/`, both under $XDG_DATA_HOME."""
    return data_home / "quick-launcher"


def core_path(data_home: Path) -> Path:
    return data_dir(data_home) / "bin" / CORE_BIN


def bundled_plugins_dir(data_home: Path) -> Path:
    return data_dir(data_home) / "plugins"


def check_cargo() -> str:
    cargo = shutil.which("cargo")
    if cargo is None:
        raise FileNotFoundError(
            "cargo is not installed. Install Rust (https://rustup.rs, or "
            "`sudo pacman -S rustup && rustup default stable`), then run install.py again.")
    return cargo


def build(cargo: str, source: Path = ROOT) -> Path:
    """Build the core and every bundled plugin's binary in release mode.
    Returns the folder holding the binaries."""
    bins = [CORE_BIN] + [manifest["bin"] for _, manifest in plugin_layout.bundled_plugins(source)]
    command = [cargo, "build", "--release", "--locked"]
    for name in bins:
        command += ["--bin", name]
    print(f"Building: {' '.join(command[1:])}", flush=True)
    subprocess.run(command, cwd=source, check=True)
    return source / "target/release"


def install_core(data_home: Path, bin_dir: Path, source: Path = ROOT) -> None:
    """Copy the core binary and lay out the bundled plugins, each swapped in
    whole so a running core never starts a half-copied file."""
    binary = Path(bin_dir) / CORE_BIN
    if not binary.is_file():
        raise FileNotFoundError(f"{binary} is missing; the build didn't produce it")
    target = core_path(data_home)
    target.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(dir=target.parent, prefix=f".{CORE_BIN}-")
    os.close(fd)
    try:
        shutil.copy2(binary, temporary)
        os.chmod(temporary, 0o755)
        os.replace(temporary, target)
    finally:
        Path(temporary).unlink(missing_ok=True)

    plugins = bundled_plugins_dir(data_home)
    staged = Path(tempfile.mkdtemp(dir=plugins.parent, prefix=".plugins-"))
    try:
        plugin_layout.lay_out(staged / "plugins", bin_dir, source)
        if plugins.exists():
            retired = plugins.with_name(f".plugins-old-{stamp()}")
            plugins.rename(retired)
            (staged / "plugins").rename(plugins)
            shutil.rmtree(retired)
        else:
            (staged / "plugins").rename(plugins)
    finally:
        shutil.rmtree(staged, ignore_errors=True)


def uninstall_core(data_home: Path) -> list:
    """Remove the core and the bundled plugins. The user's plugins, config
    and state live elsewhere and stay."""
    removed = []
    for path in (core_path(data_home), bundled_plugins_dir(data_home)):
        if path.is_dir():
            shutil.rmtree(path)
            removed.append(path)
        elif path.exists():
            path.unlink()
            removed.append(path)
    for folder in (core_path(data_home).parent, data_dir(data_home)):
        try:
            folder.rmdir()
        except OSError:
            pass
    return removed


def install_files(config_home: Path, source: Path = ROOT) -> bool:
    """Copy the shell plugin into place. Returns whether an older copy was
    replaced."""
    source = source / FRONTEND
    for name in PLUGIN_FILES:
        if not (source / name).is_file():
            raise FileNotFoundError(f"Missing plugin file: {source / name}")
    target = plugin_dir(config_home)
    target.parent.mkdir(parents=True, exist_ok=True)
    # Stage beside the target and swap, so the shell's plugin watcher never
    # loads a half-copied plugin.
    staged = Path(tempfile.mkdtemp(dir=target.parent, prefix=f".{PLUGIN_ID}-"))
    try:
        for name in PLUGIN_FILES:
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


def install(config_home: Path, data_home: Path, bin_dir: Path) -> list:
    """Install from binaries already built in `bin_dir`."""
    messages = []
    install_core(data_home, bin_dir)
    messages.append(f"Installed {CORE_BIN} in {core_path(data_home).parent}")
    messages.append(f"Installed the bundled plugins in {bundled_plugins_dir(data_home)}")
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
        # keepLoaded plugins stay mounted across hot reloads, and keep the
        # core they started.
        messages.append("Updated an existing install: run `omarchy restart shell` to load it.")
    return messages


def uninstall(config_home: Path, data_home: Path) -> list:
    messages = []
    target = plugin_dir(config_home)
    if target.exists():
        shutil.rmtree(target)
        messages.append(f"Removed {target}")
    for path in uninstall_core(data_home):
        messages.append(f"Removed {path}")
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
    parser.add_argument(
        "--data-home", type=Path,
        default=Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share"),
        help="Data directory for the core and bundled plugins (default: $XDG_DATA_HOME or "
             "~/.local/share). The launcher looks for the core there, so change it only for testing.",
    )
    parser.add_argument("--uninstall", action="store_true", help="Remove the launcher")
    parser.add_argument("--no-reload", action="store_true",
                        help="Don't ask the running shell to reload plugins")
    args = parser.parse_args()
    try:
        if args.uninstall:
            messages = uninstall(args.config_home, args.data_home)
        else:
            messages = install(args.config_home, args.data_home, build(check_cargo()))
    except subprocess.CalledProcessError as error:
        parser.exit(1, f"Install failed: the build exited with status {error.returncode}\n")
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f"{'Uninstall' if args.uninstall else 'Install'} failed: {error}\n")
    print("\n".join(messages))
    if not args.no_reload:
        reload_desktop()


if __name__ == "__main__":
    main()
