# Quick launcher

A keyboard launcher for the Omarchy shell that replaces the apps menu. With
no prefix it searches applications, just like the stock menu. Typing a mode prefix switches the suggestions to that mode:

| Prefix          | Mode     | Suggestions                                        | Enter                      | Shift+Enter / right-click |
|-----------------|----------|----------------------------------------------------|----------------------------|---------------------------|
| *(none)*        | Apps     | Installed applications                             | Launch the app             | —                         |
| `pi:`           | pi       | Pinned, then recently used pi projects             | Open pi in the project     | Pin / unpin               |
| `>` or `term:`  | Terminal | The typed command, then commands run before        | Run it in a new terminal   | Forget a past command     |
| `cd:` or `proj:`| Projects | Saved project folders, most recently used first    | Open a terminal there      | Remove it (also Ctrl+D)   |

Typing part of a mode's prefix or name (`p`, `term`) suggests the mode, and
**Tab** jumps into it. **Backspace** on an empty query leaves the mode. Up/Down
(or Ctrl+N/P, Ctrl+J/K) move the selection and Escape clears the query, then
closes.

The pi mode reads recent projects from pi's session history, including
`PI_CODING_AGENT_DIR`, `PI_CODING_AGENT_SESSION_DIR` and `sessionDir`.
Pinned folders that no longer exist are dimmed and marked *missing*. Terminal
commands run in an interactive bash (so aliases work) in the focused terminal's
folder, and the terminal stays open afterwards.

The projects mode only opens folders you've saved. Typing a name, or part of
a path, searches the saved projects. To add one, pick **Add a project
folder…** (or press Tab) or type a path starting with `~` or `/`: the list
switches to the folders that path completes to, as `cd` does in a shell.
**Tab** completes the selected folder, Enter saves it, and Shift+Enter saves
it and opens a terminal there. Folders starting with `.` show once you type
the `.`.

## How it works

The launcher is a Rust program, `quick-launcher-core`, plus plugins. The core
finds the plugins, reads prefixes, filters and ranks every item, and runs the
action you pick. Each mode is a plugin: `apps`, `pi`, `projects` and `terminal` are
bundled, and you can add your own. The Omarchy shell plugin
(`frontends/omarchy/Launcher.qml`) only draws what the core sends and passes
on what you type. [docs/protocol.md](docs/protocol.md) is the full design.

## Install

Requires a recent Omarchy (Quickshell shell plugins, Lua Hyprland config), a
Rust toolchain (`cargo`, from [rustup](https://rustup.rs) or
`sudo pacman -S rustup && rustup default stable`), Python 3, and `pi` on your
`PATH`.

```sh
git clone https://github.com/abhi-abhiram/quick-launcher.git
cd quick-launcher
python3 install.py
```

The installer:

- builds the core and the bundled plugins with `cargo build --release`
- installs the core to `~/.local/share/quick-launcher/bin/` and the bundled
  plugins to `~/.local/share/quick-launcher/plugins/<id>/`
  (under `$XDG_DATA_HOME` when it's set)
- copies the shell plugin to `~/.config/omarchy/plugins/abhiram.quick-launcher/`
  and enables it in `~/.config/omarchy/shell.json`
- removes the old [pi-launcher](https://github.com/abhi-abhiram/pi-launcher)
  bar button, if present. Its pinned projects carry over.

It backs up `shell.json` before changing it and can be run again safely, so
updating is `git pull` and `python3 install.py`. The shell keeps a loaded
launcher, and the core it started, across hot reloads, so after an update run
`omarchy restart shell` (the installer reminds you).

The installer leaves your keybindings alone. Bind a key the usual Omarchy
way in `~/.config/hypr/bindings.lua`, unbinding the default first. For example,
to put the launcher on Super+Space and move the Omarchy menu to
Super+Alt+Space:

```lua
hl.unbind("SUPER + SPACE")
hl.unbind("SUPER + ALT + SPACE")
o.bind("SUPER + SPACE", "Quick launcher", "omarchy-shell shell toggle abhiram.quick-launcher '{}'")
o.bind("SUPER + ALT + SPACE", "Omarchy menu", "omarchy-menu toggle")
```

A payload such as `'{"mode":"pi"}'` opens the launcher straight into a mode
(the payload names a plugin id).

To uninstall, run `python3 install.py --uninstall` and remove the binding.
Your pins, history, config and own plugins stay.

## Data

Each plugin keeps its files in its own folders:

- Pinned pi projects: `~/.config/quick-launcher/pi/pinned.json`
  (`{"projects": ["/path/to/project"]}`), editable by hand
- Terminal history: `~/.local/state/quick-launcher/terminal/history.json`
- Saved projects: `~/.local/state/quick-launcher/projects/projects.json`
  (`{"projects": ["/path/to/project"]}`, most recent first)
- Your plugins: `~/.config/quick-launcher/plugins/<id>/`
- Settings: `~/.config/quick-launcher/config.toml`, for example:

  ```toml
  disabled = ["terminal"]          # plugin ids to skip

  [plugins.pi]
  prefixes = ["p:", "pi:"]         # replaces the plugin's own prefixes
  ```

Pins and history from earlier versions (`pi-pinned.json`,
`terminal-history.json`) are read until the plugin first writes its own file,
which carries them forward. The old files are left untouched.

## Writing a plugin

A plugin is a folder in `~/.config/quick-launcher/plugins/` holding a
`plugin.toml` and one binary built with the plugin SDK,
[`crates/plugin`](crates/plugin) (`quick-launcher-plugin`). It takes effect
the next time the launcher opens; nothing needs rebuilding or restarting.

```toml
# ~/.config/quick-launcher/plugins/links/plugin.toml
protocol = 1
id = "links"
name = "Links"
icon = "󰌷"
prefixes = ["link:"]
bin = "links"
```

```rust
use quick_launcher_plugin::{Action, Item, Plugin, Run};

struct Links;

impl Plugin for Links {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        let url = "https://github.com/abhi-abhiram/quick-launcher/issues";
        Ok(vec![Item::new(url, "Quick launcher issues")
            .detail(url)
            .action(Action::new("open", "Open").run(Run::open(url)))
            .action(Action::new("copy", "Copy link").run(Run::copy(url)))])
    }
}

fn main() { quick_launcher_plugin::run(Links) }
```

`link:` then lists that item, Enter opens it and Shift+Enter copies the link.
A fuller example, listing the git repos under `~/Projects`, is
[`crates/plugin/examples/repos.rs`](crates/plugin/examples/repos.rs). Build
your plugin with `cargo build --release` and copy the binary next to its
`plugin.toml`. [docs/protocol.md](docs/protocol.md) covers the manifest
(kinds, lifetimes, `[query_item]`), items, actions and the SDK. The bundled
plugins in [`plugins/`](plugins) are worked examples too.

## Development

```sh
python3 tools/check.py
```

runs everything a PR must pass: `cargo fmt --check`, `cargo clippy` (also
with the `recording-platform` feature), `cargo test`, the core's end-to-end
test (`tests/e2e_core.py`) and the installer's tests. The CI workflow runs
the same checks on every PR and every push to `main`. The shell plugin is
checked with:

```sh
qmllint frontends/omarchy/Launcher.qml
omarchy plugin validate "$PWD/frontends/omarchy"
```

To try the core without the shell, run `python3 tools/dev.py`. It builds the
workspace, lays out the bundled plugins (and the `repos` example) in
`target/plugins/` and starts the core as a minimal terminal frontend: type a
line to search, `!1` to run the first item's default action (`!!1` for its
alternate), `<` to leave a mode and `\t` to complete one. It uses the real
platform, so `!1` on an app launches it.

Tests use temporary directories and never launch a terminal or touch your
desktop. To try an installer change without touching your setup, use
`python3 install.py --config-home /tmp/ql/config --data-home /tmp/ql/data --no-reload`.

[docs/milestones.md](docs/milestones.md) records how v1 was built, one
milestone per PR.
