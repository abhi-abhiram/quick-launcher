#!/usr/bin/env python3
"""Install the quick launcher: build the core and the bundled plugins, then
install them with the frontend that draws the launcher: the Omarchy shell
plugin on Linux, the Quick Launcher menu bar app on macOS."""

import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import plistlib
import re
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

# The macOS app, built with SwiftPM from MAC_FRONTEND.
MAC_FRONTEND = Path("frontends/macos")
MAC_APP_NAME = "Quick Launcher.app"
MAC_EXECUTABLE = "QuickLauncher"
MAC_BUNDLE_ID = "io.github.abhi-abhiram.quick-launcher"
MAC_HOTKEY_HINT = f"""Press Option+Space to open the launcher. To use another key, for example:
  defaults write {MAC_BUNDLE_ID} hotkey "ctrl+space"
then quit Quick Launcher from its menu bar icon and open it again."""

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


def target_dir(source: Path = ROOT, environ=os.environ) -> Path:
    """Cargo's target folder: $CARGO_TARGET_DIR (relative to `source`) when
    set, `source/target` otherwise. `build` passes it to cargo explicitly,
    so the binaries are where this says."""
    return source / (environ.get("CARGO_TARGET_DIR") or "target")


def build(cargo: str, source: Path = ROOT, environ=os.environ) -> Path:
    """Build the core and every bundled plugin's binary in release mode.
    Returns the folder holding the binaries."""
    bins = [CORE_BIN] + [manifest["bin"] for _, manifest in plugin_layout.bundled_plugins(source)]
    target = target_dir(source, environ)
    command = [cargo, "build", "--release", "--locked", "--target-dir", str(target)]
    for name in bins:
        command += ["--bin", name]
    print(f"Building: {' '.join(command[1:])}", flush=True)
    subprocess.run(command, cwd=source, check=True)
    return target / "release"


def install_core(data_home: Path, bin_dir: Path, source: Path = ROOT) -> None:
    """Copy the core binary and lay out the bundled plugins, each swapped in
    whole so a running core never starts a half-copied file.

    Both are staged before either is swapped in, so a missing binary leaves
    the installed core and plugins as they were rather than mixing versions.
    """
    binary = Path(bin_dir) / CORE_BIN
    if not binary.is_file():
        raise FileNotFoundError(f"{binary} is missing; the build didn't produce it")
    target = core_path(data_home)
    target.parent.mkdir(parents=True, exist_ok=True)
    plugins = bundled_plugins_dir(data_home)
    staged = Path(tempfile.mkdtemp(dir=plugins.parent, prefix=".plugins-"))
    fd, temporary = tempfile.mkstemp(dir=target.parent, prefix=f".{CORE_BIN}-")
    os.close(fd)
    try:
        plugin_layout.lay_out(staged / "plugins", bin_dir, source)
        shutil.copy2(binary, temporary)
        os.chmod(temporary, 0o755)

        os.replace(temporary, target)
        if plugins.exists():
            retired = plugins.with_name(f".plugins-old-{stamp()}")
            plugins.rename(retired)
            (staged / "plugins").rename(plugins)
            shutil.rmtree(retired)
        else:
            (staged / "plugins").rename(plugins)
    finally:
        Path(temporary).unlink(missing_ok=True)
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


# ---------------------------------------------------------------- macOS


def workspace_version(source: Path = ROOT) -> str:
    """The workspace version in Cargo.toml, which the app reports too."""
    text = (source / "Cargo.toml").read_text()
    match = re.search(r'^\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"', text, re.M | re.S)
    return match[1] if match else "0.0.0"


def check_swift() -> str:
    swift = shutil.which("swift")
    if swift is None:
        raise FileNotFoundError(
            "swift is not installed. Install the Xcode command line tools "
            "(`xcode-select --install`), then run install.py again.")
    return swift


def build_mac_app(swift: str, source: Path = ROOT) -> Path:
    """Build the macOS app's executable in release mode. Returns its path."""
    package = source / MAC_FRONTEND
    command = [swift, "build", "-c", "release", "--package-path", str(package)]
    print(f"Building: {' '.join(command[1:])}", flush=True)
    subprocess.run(command, check=True)
    shown = subprocess.run(command + ["--show-bin-path"], check=True, capture_output=True, text=True)
    return Path(shown.stdout.strip()) / MAC_EXECUTABLE


def info_plist(version: str) -> dict:
    """The app's Info.plist: a menu bar app (no Dock icon) that opens
    quick-launcher:// URLs."""
    return {
        "CFBundleIdentifier": MAC_BUNDLE_ID,
        "CFBundleName": "Quick Launcher",
        "CFBundleDisplayName": "Quick Launcher",
        "CFBundleExecutable": MAC_EXECUTABLE,
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "CFBundleInfoDictionaryVersion": "6.0",
        "LSMinimumSystemVersion": "13.0",
        "LSUIElement": True,
        "NSHighResolutionCapable": True,
        "CFBundleURLTypes": [{
            "CFBundleURLName": MAC_BUNDLE_ID,
            "CFBundleURLSchemes": ["quick-launcher"],
        }],
    }


def mac_app_path(app_dir: Path) -> Path:
    return app_dir / MAC_APP_NAME


def install_mac_app(app_dir: Path, executable: Path, version: str) -> Path:
    """Lay out the app bundle and swap it in whole. Returns its path."""
    executable = Path(executable)
    if not executable.is_file():
        raise FileNotFoundError(f"{executable} is missing; the build didn't produce it")
    target = mac_app_path(app_dir)
    app_dir.mkdir(parents=True, exist_ok=True)
    staged = Path(tempfile.mkdtemp(dir=app_dir, prefix=".quick-launcher-"))
    try:
        bundle = staged / MAC_APP_NAME
        (bundle / "Contents/MacOS").mkdir(parents=True)
        shutil.copy2(executable, bundle / "Contents/MacOS" / MAC_EXECUTABLE)
        os.chmod(bundle / "Contents/MacOS" / MAC_EXECUTABLE, 0o755)
        with open(bundle / "Contents/Info.plist", "wb") as stream:
            plistlib.dump(info_plist(version), stream)
        if target.exists():
            retired = target.with_name(f".{MAC_APP_NAME}-old-{stamp()}")
            target.rename(retired)
            bundle.rename(target)
            shutil.rmtree(retired)
        else:
            bundle.rename(target)
        return target
    finally:
        shutil.rmtree(staged, ignore_errors=True)


def sign_mac_app(app: Path) -> None:
    """Sign the bundle ad hoc, which Apple silicon and Open at Login need."""
    command = ["codesign", "--force", "--sign", "-", str(app)]
    try:
        subprocess.run(command, check=True, capture_output=True)
    except (OSError, subprocess.CalledProcessError) as error:
        print(f"Could not sign {app}: {error}")


def restart_mac_app(app: Path) -> None:
    """Quit a running copy, so the new one and its core take over, and open
    the installed one."""
    subprocess.run(["pkill", "-x", MAC_EXECUTABLE], capture_output=True, check=False)
    try:
        subprocess.run(["open", str(app)], capture_output=True, timeout=10, check=False)
    except (OSError, subprocess.SubprocessError) as error:
        print(f"Could not open {app}: {error}")


def install_macos(data_home: Path, app_dir: Path, bin_dir: Path, executable: Path,
                  version: str) -> list:
    """Install from binaries already built: the core and bundled plugins
    under the data home, as on Linux, and the app in `app_dir`."""
    messages = []
    install_core(data_home, bin_dir)
    messages.append(f"Installed {CORE_BIN} in {core_path(data_home).parent}")
    messages.append(f"Installed the bundled plugins in {bundled_plugins_dir(data_home)}")
    app = install_mac_app(app_dir, executable, version)
    messages.append(f"Installed {app}")
    messages.append(MAC_HOTKEY_HINT)
    return messages


def uninstall_macos(data_home: Path, app_dir: Path) -> list:
    messages = []
    app = mac_app_path(app_dir)
    if app.exists():
        subprocess.run(["pkill", "-x", MAC_EXECUTABLE], capture_output=True, check=False)
        shutil.rmtree(app)
        messages.append(f"Removed {app}")
    for path in uninstall_core(data_home):
        messages.append(f"Removed {path}")
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
             "~/.local/share). The launcher always starts the core from $XDG_DATA_HOME, "
             "so set this only to try the installer.",
    )
    parser.add_argument(
        "--app-dir", type=Path, default=Path.home() / "Applications",
        help="macOS only: where Quick Launcher.app goes (default: ~/Applications)",
    )
    parser.add_argument("--uninstall", action="store_true", help="Remove the launcher")
    parser.add_argument("--no-reload", action="store_true",
                        help="Don't ask the running shell to reload plugins, or on macOS, "
                             "don't open the installed app")
    args = parser.parse_args()
    mac = sys.platform == "darwin"
    try:
        if args.uninstall and mac:
            messages = uninstall_macos(args.data_home, args.app_dir)
        elif args.uninstall:
            messages = uninstall(args.config_home, args.data_home)
        elif mac:
            bin_dir = build(check_cargo())
            executable = build_mac_app(check_swift())
            messages = install_macos(args.data_home, args.app_dir, bin_dir, executable,
                                     workspace_version())
        else:
            messages = install(args.config_home, args.data_home, build(check_cargo()))
    except subprocess.CalledProcessError as error:
        parser.exit(1, f"Install failed: the build exited with status {error.returncode}\n")
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f"{'Uninstall' if args.uninstall else 'Install'} failed: {error}\n")
    print("\n".join(messages))
    if mac and not args.uninstall:
        sign_mac_app(mac_app_path(args.app_dir))
        if not args.no_reload:
            restart_mac_app(mac_app_path(args.app_dir))
    elif not args.no_reload and not mac:
        reload_desktop()


if __name__ == "__main__":
    main()
