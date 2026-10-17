# Quick launcher

A keyboard launcher for the Omarchy shell that replaces the apps menu. With
no prefix it searches applications, just like the stock menu. Typing a mode prefix switches the suggestions to that mode:

| Prefix          | Mode     | Suggestions                                        | Enter                      | Shift+Enter / right-click |
|-----------------|----------|----------------------------------------------------|----------------------------|---------------------------|
| *(none)*        | Apps     | Installed applications                             | Launch the app             | —                         |
| `pi:`           | pi       | Pinned, then recently used pi projects             | Open pi in the project     | Pin / unpin               |
| `>` or `term:`  | Terminal | The typed command, then commands run before        | Run it in a new terminal   | Forget a past command     |

Typing part of a mode's prefix or name (`p`, `term`) suggests the mode, and
**Tab** jumps into it. **Backspace** on an empty query leaves the mode. Up/Down
(or Ctrl+N/P, Ctrl+J/K) move the selection and Escape clears the query, then
closes.

The pi mode reads recent projects from pi's session history, including
`PI_CODING_AGENT_DIR`, `PI_CODING_AGENT_SESSION_DIR` and `sessionDir`.
Pinned folders that no longer exist are dimmed and marked *missing*. Terminal
commands run in an interactive bash (so aliases work) in the focused terminal's
folder, and the terminal stays open afterwards.

## Install

Requires a recent Omarchy (Quickshell shell plugins, Lua Hyprland config),
Python 3, and `pi` on your `PATH`.

```sh
git clone https://github.com/abhi-abhiram/quick-launcher.git
cd quick-launcher
python3 install.py
```

The installer:

- copies the plugin to `~/.config/omarchy/plugins/abhiram.quick-launcher/`
  and enables it in `~/.config/omarchy/shell.json`
- removes the old [pi-launcher](https://github.com/abhi-abhiram/pi-launcher)
  bar button, if present. Its pinned projects carry over.

It backs up `shell.json` before changing it and can be run again safely. The shell keeps a loaded launcher across hot reloads, so after an
update run `omarchy restart shell` (the installer reminds you).

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

A payload such as `'{"mode":"pi"}'` opens the launcher straight into a mode.

To uninstall, run `python3 install.py --uninstall` and remove the binding.

## Data

- Pinned pi projects: `~/.config/quick-launcher/pi-pinned.json`
  (`{"projects": ["/path/to/project"]}`), editable by hand
- Terminal history: `~/.local/state/quick-launcher/terminal-history.json`

## Adding a mode

Modes are declared in `modes.json`. Each one names its prefixes and a `list`
command, which runs from the plugin folder in a login shell and prints:

```json
{"rows": [{
  "label": "Shown in bold",
  "detail": "Shown underneath",
  "icon": "",
  "action": ["argv", "run", "on", "Enter"],
  "secondary": {"label": "Pin", "action": ["argv", "run", "on", "Shift+Enter"]},
  "disabled": false
}]}
```

The launcher runs each list command when it opens and filters the rows itself
as you type, so typing never waits on a process. Actions are argv arrays and
never pass through a shell. Enter runs `action` and closes the launcher.
A `secondary` action keeps the launcher open and reloads the mode afterwards.
A mode can also declare a `query` row that acts on the typed text itself. The
terminal mode uses one, and `{query}` in its label, detail, or action is
replaced by the text.

## Development

```sh
python3 -m unittest discover -s tests -v
node --test tests/test_model.mjs
qmllint Launcher.qml
omarchy plugin validate "$PWD"
```

The v1 core and plugin SDK ([docs/protocol.md](docs/protocol.md)) are being
built next to v0 as a Cargo workspace under `crates/`, following
[docs/milestones.md](docs/milestones.md). Nothing in the shell uses them yet.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

`python3 tools/check.py` runs all of the above, v0's tests included. Run it
before pushing: the GitHub Actions workflow runs the same checks but only
when started by hand.

Tests use temporary directories and never launch a terminal or touch your
desktop. To try an installer change without touching your config, use
`python3 install.py --config-home /path/to/config --no-reload`.

### Omarchy app library workaround

Omarchy is meant to hand menu plugins its shared app library. In omarchy 4.0.4
the check fails for third-party plugins: the manifest's `kinds` reaches it
through an `Instantiator` model, where `Array.isArray()` returns false. The
launcher therefore runs its own copy of Omarchy's `AppLibrary` service until
the shell provides one.
