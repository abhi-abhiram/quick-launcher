#!/usr/bin/env python3
"""Lay out the bundled plugins as plugin folders the core can discover.

Each folder in `plugins/` holds a `plugin.toml` and a Cargo package whose
binary is the manifest's `bin`. A plugin folder is `<dest>/<id>/` with the
manifest and that binary, built beforehand by `cargo build`.

`tools/dev.py` uses this for `target/plugins/`; `install.py` will use it
for the installed plugins.
"""

import shutil
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def bundled_plugins(root=ROOT):
    """Each bundled plugin's source folder and manifest, sorted by folder."""
    plugins = []
    for manifest in sorted((root / "plugins").glob("*/plugin.toml")):
        with manifest.open("rb") as f:
            plugins.append((manifest.parent, tomllib.load(f)))
    return plugins


def lay_out(dest, bin_dir, root=ROOT):
    """Replace `dest` with one folder per bundled plugin, its binary copied
    from `bin_dir`. Returns the plugin ids laid out."""
    dest = Path(dest)
    if dest.exists():
        shutil.rmtree(dest)
    dest.mkdir(parents=True)
    ids = []
    for source, manifest in bundled_plugins(root):
        plugin_id, bin_name = manifest["id"], manifest["bin"]
        if source.name != plugin_id:
            raise ValueError(f"{source}: the folder must be named after the plugin id {plugin_id!r}")
        binary = Path(bin_dir) / bin_name
        if not binary.is_file():
            raise FileNotFoundError(f"{binary} is missing; build the workspace first")
        folder = dest / plugin_id
        folder.mkdir()
        shutil.copy2(source / "plugin.toml", folder / "plugin.toml")
        shutil.copy2(binary, folder / bin_name)
        ids.append(plugin_id)
    return ids


def main():
    profile = "release" if "--release" in sys.argv[1:] else "debug"
    target = ROOT / "target"
    ids = lay_out(target / "plugins", target / profile)
    print(f"laid out {', '.join(ids)} in {target / 'plugins'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
