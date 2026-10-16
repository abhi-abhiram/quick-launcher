# Quick launcher architecture and protocols (v1, draft)

Status: **draft for review**. Nothing here is implemented yet. It replaces the
`modes.json` contract described in the README ("v0"), which keeps working
until every bundled mode is ported; see [Migrating from v0](#migrating-from-v0).

The keywords MUST, SHOULD and MAY are used as in RFC 2119.

## Goals

- **Adding a plugin is dropping in a folder.** A manifest plus one executable,
  in any language. No launcher changes, no rebuild, no shell restart.
- **Typing never waits on a process.** Most plugins list their items once and
  the core filters them. Plugins that must see every keystroke stay running,
  so there is no startup cost per keystroke.
- **Nothing passes through a shell.** Actions are argv arrays.
- **The core is UI- and platform-agnostic.** Omarchy is the only supported
  platform for now, but nothing outside the UI and one platform module knows
  that. A macOS UI later is a new window plus a second platform module.
- **Rust first.** The core, the bundled plugins and the plugin SDK are Rust.
  Plugins only need to speak JSON over stdio, so a shell or Python plugin
  still works.

## Architecture

```
┌──────────────┐  UI protocol   ┌────────────────────┐  plugin protocol  ┌───────────────┐
│ UI           │◄──────────────►│ core               │◄─────────────────►│ pi plugin     │
│ (Omarchy QML)│  JSON over     │ (Rust)             │  JSON over stdio, ├───────────────┤
│ window, keys,│  stdio         │ plugins, prefixes, │  one process per  │ terminal      │
│ apps search  │                │ matching, actions  │  plugin           ├───────────────┤
└──────────────┘                │ ┌────────────────┐ │                   │ user plugins… │
                                │ │ Platform:      │ │                   └───────────────┘
                                │ │ Omarchy        │ │
                                │ └────────────────┘ │
                                └────────────────────┘
```

- **UI**: draws the window, handles keys, searches apps and launches them.
  It holds no launcher logic: it sends what was typed or chosen and draws the
  state the core sends back. The only UI today is the Omarchy shell plugin
  (`Launcher.qml`).
- **Core**: `quick-launcher-core`, one Rust binary. It discovers plugins,
  parses prefixes, runs plugin processes, filters and ranks items, and runs
  actions through its platform module.
- **Platform**: a module inside the core holding everything OS-specific. See
  [Platform](#platform).
- **Plugin**: a folder with a `plugin.toml` manifest and an executable that
  speaks the [plugin protocol](#plugin-protocol).

### Why apps stay in the UI

Omarchy's shell already has an app library with icons, hidden entries, launch
feedback and uninstall. Rewriting that in the core would be more work and a
worse result on Omarchy. So apps are the one search the UI does itself, and
the core marks where its own items go relative to the apps (see
[`placement`](#ui-items)). A later macOS UI can supply macOS apps the same way,
or a Rust apps plugin can replace both; the core doesn't change either way.

### Rules that keep the core agnostic

- The core MUST NOT depend on Quickshell, Hyprland, Omarchy or any UI
  toolkit. Its crate builds and its tests pass on a machine without them.
- Every OS call (starting processes detached, terminals, opening URLs, the
  clipboard, config and state paths, login environments) goes through the
  `Platform` trait. No other core module calls OS tools directly.
- Nothing plugin-specific lives in the core. The core never knows what pi is.
- There is exactly one `Platform` implementation and one UI until a second is
  actually being built. No stubs, no `cfg` branches for platforms that don't
  exist yet.

## UI protocol

The UI starts the core once, as a long-running child process, and keeps it
for the life of the shell (the Omarchy plugin is `keepLoaded`). They exchange
JSON Lines over the core's stdin and stdout; the core's stderr is the log.
Every message has `v` (protocol version) and `type`.

### UI → core

| type       | fields | meaning |
|------------|--------|---------|
| `hello`    | `ui` (string), `version` (string) | First message. The core replies `ready`. |
| `open`     | `mode` (optional plugin id) | The window was shown, optionally straight into a mode. The core resets to that mode (or none) with an empty query, refreshes list plugins as needed, and sends `state`. |
| `input`    | `seq` (int), `text` (string) | The search field changed. `text` is the whole field, which in a mode excludes the prefix. `seq` increases with every `input`. |
| `back`     | — | Backspace on an empty field. Leaves the mode. |
| `complete` | — | Tab. Enters the mode of the first mode hint, if any. |
| `activate` | `item` (string), `action` (string) | Run an action of a core item. `item` is the item's `ref`, `action` an action `id`. |
| `close`    | — | The window was hidden. |

Closing stdin ends the core.

### Core → UI

| type    | fields | meaning |
|---------|--------|---------|
| `ready` | `protocol` (int), `version` (string) | Reply to `hello`. |
| `state` | see below | The complete state to draw. Sent after every message that changes it and whenever a plugin answers. |
| `close` | — | Hide the window (an action ran and asked to close). |

```json
{"v":1,"type":"state","seq":12,
 "mode":{"id":"pi","name":"pi","icon":"π","placeholder":"Open pi in…"},
 "text":"launch","loading":false,"error":null,
 "items":[ ... ]}
```

| field     | meaning |
|-----------|---------|
| `seq`     | The newest `input` the core has applied. |
| `mode`    | The active mode, or `null` for the plain search. |
| `text`    | The search field's content. Differs from what the UI sent when the core changed mode, e.g. typing `pi:launch` gives mode `pi` and text `launch`. |
| `loading` | A plugin request for this mode and text is still outstanding. |
| `error`   | A one-line, user-readable message, or `null`. |
| `items`   | Items to show, best first. In the plain search these are only the core's own items; the UI merges them with apps. |

**Keeping the field in sync.** A user can type faster than the core answers.
The UI MUST NOT copy `text` into the field from a `state` whose `seq` is lower
than the last `seq` it sent. It still draws that state's items, which the
core recomputes for its own newest text anyway.

### UI items

What the UI needs to draw a row, with no knowledge of how actions run:

```json
{"ref":"pi/~/Projects/quick-launcher","label":"quick-launcher",
 "detail":"~/Projects/quick-launcher","icon":{"glyph":"π"},"badge":"pinned",
 "disabled":false,"placement":"after_apps",
 "actions":[{"id":"open","title":"Open pi"},{"id":"unpin","title":"Unpin","key":"ctrl+p"}]}
```

- `ref` identifies the item to the core; the UI treats it as opaque.
- `icon` is always an explicit object (`glyph`, `name` or `path`); the core
  has already resolved the shorthand forms plugins may use.
- `placement` matters only in the plain search: `before_apps` or
  `after_apps`. Mode hints are `before_apps` when the text names the mode
  outright (`pi`, `term`), otherwise `after_apps`.
- Key bindings are the same as for plugins: Enter runs the first action,
  Shift+Enter the second, and an action's `key` its own shortcut.

## Plugin protocol

### Plugin discovery

The core reads plugins from, in order:

1. the bundled plugin directory, `$XDG_DATA_HOME/quick-launcher/plugins/<id>/`
2. the user plugin directory, `$XDG_CONFIG_HOME/quick-launcher/plugins/<id>/`

These are the Omarchy (Linux) paths; the core gets them from `Platform`. A
user plugin with the same `id` as a bundled one replaces it. The folder name
MUST equal the manifest `id`. Both directories are watched; adding, editing
or removing a plugin takes effect the next time the launcher opens.

User overrides live in `config.toml` in the config directory:

```toml
disabled = ["terminal"]          # plugin ids to skip

[plugins.pi]
prefixes = ["p:", "pi:"]         # replaces the manifest's prefixes
```

If two enabled plugins claim the same prefix, the one discovered first keeps
it and the core logs a warning naming both.

### Manifest: `plugin.toml`

```toml
protocol = 1                      # required; major protocol version
id = "pi"                         # required; [a-z0-9-]+, unique
name = "pi"                       # required; shown in hints and the mode chip
description = "Open pi in a project"
icon = "π"                        # see Icons
prefixes = ["pi:"]                # required; at least one, matched case-insensitively
placeholder = "Open pi in…"       # shown in the empty search field
exec = ["./pi-plugin"]            # required; argv, relative paths resolve
                                  #   against the plugin folder
kind = "list"                     # "list" (default) or "query"
lifetime = "oneshot"              # "oneshot" (default) or "persistent"
env = "session"                   # "session" (default) or "login"; see Environment
timeout_ms = 2000                 # per request; default 2000

# Optional: an item built from the typed text, shown first. Only for
# kind = "list". `{query}` is replaced in every string, including argv.
[query_item]
label = "{query}"
detail = "Run in a new terminal"
actions = [{ title = "Run", run = { plugin = "run" } }]
```

Unknown keys MUST be ignored, so newer manifests load in older cores.

### Kinds

- **`list`**: the core sends one `list` request when the mode is entered (and
  again each time the launcher opens while in it). The plugin returns every
  item, and the core filters and ranks them as you type. Use this whenever
  the full set is small enough to send at once (up to a few thousand items).
  pi projects and terminal history are list plugins.
- **`query`**: the core sends a `query` request on every keystroke, and the
  plugin returns items for that text, already filtered and ranked. Use this
  when the items depend on the text (calculator, web search, file search over
  a large tree). Query plugins SHOULD be `persistent`.

### Lifetimes

- **`oneshot`**: the core starts the executable, writes one request line,
  closes stdin, reads one response line and waits for it to exit. A plugin
  that needs no input MAY ignore stdin entirely and just print its response.
- **`persistent`**: the core starts the executable on first use and keeps it
  running, writing one request per line and reading one response per line.
  When the core exits it closes stdin, and the plugin MUST exit when stdin
  reaches EOF.

### Transport

- JSON Lines over stdio: one UTF-8 JSON object per line, `\n` terminated.
- stdin carries requests, stdout carries responses. A plugin MUST NOT write
  anything else to stdout.
- stderr is for logs. The core forwards it to its own log, prefixed with the
  plugin id.
- A line MUST NOT exceed 4 MiB.

### Requests

Every request has `v` (protocol version), `id` (an integer, unique per plugin
process) and `method`.

| method     | sent to                    | params                         |
|------------|----------------------------|--------------------------------|
| `list`     | `list` plugins             | `{}`                           |
| `query`    | `query` plugins            | `{"text": "what was typed"}`   |
| `activate` | any plugin, for a `plugin` action | `{"item": "<item id>", "action": "<action id>", "text": "..."}` |

```json
{"v":1,"id":7,"method":"query","params":{"text":"2*21"}}
```

`text` is what follows the prefix, with leading whitespace removed.

### Responses

Every response echoes the request `id`. A `oneshot` plugin MAY omit it.

```json
{"id":7,"items":[ ... ]}
{"id":8,"result":{"close":true}}
{"id":9,"error":{"message":"pinned.json is not valid JSON"}}
```

- `list` and `query` return `items`.
- `activate` returns `result`:
  - `close` (bool, default `true`): close the launcher.
  - `refresh` (bool, default `false`): re-request the mode's items. Implies
    the launcher stays open.
  - `query` (string, optional): replace the typed text.
- `error` becomes the state's `error`, shown in place of the items. The
  message SHOULD be one short, user-readable sentence.

For a `persistent` plugin the core may send a new `query` before the previous
one is answered. It uses only the response to the newest request and drops
the rest, so plugins MAY answer in order without tracking cancellation.

### Items

```json
{
  "id": "/home/abhiram/Projects/quick-launcher",
  "label": "quick-launcher",
  "detail": "~/Projects/quick-launcher",
  "icon": "󰐃",
  "badge": "pinned",
  "keywords": ["launcher"],
  "disabled": false,
  "actions": [
    {"id": "open", "title": "Open pi",
     "run": {"terminal": ["pi"], "cwd": "/home/abhiram/Projects/quick-launcher"}},
    {"id": "unpin", "title": "Unpin", "run": {"plugin": "unpin"}, "after": "refresh"}
  ]
}
```

| field      | type     | required | meaning |
|------------|----------|----------|---------|
| `id`       | string   | yes      | Stable within the plugin. Sent back in `activate`. |
| `label`    | string   | yes      | Main line. Filtered on. |
| `detail`   | string   | no       | Second line. Filtered on, ranked below label matches. |
| `icon`     | string \| object | no | See Icons. Defaults to the plugin icon. |
| `badge`    | string   | no       | Short tag after the label, e.g. `last used`, `missing`. |
| `keywords` | string[] | no       | Extra filter terms, not shown. |
| `disabled` | bool     | no       | Shown dimmed; the first action cannot run. Other actions can (so a missing pinned folder can still be unpinned). |
| `score`    | number   | no       | `query` plugins only: higher ranks first. Otherwise list order is kept. |
| `actions`  | Action[] | yes      | At least one. |

The core turns each item into a [UI item](#ui-items), with `ref` built from
the plugin id and the item `id`.

### Actions

The first action runs on **Enter** (or click). The second runs on
**Shift+Enter** (or right-click). An action MAY set `key` (for example
`"ctrl+d"`) to get its own shortcut. The selected row shows its non-default
actions and their keys.

| field   | type   | meaning |
|---------|--------|---------|
| `id`    | string | Sent back in `activate`. Defaults to the action's index. |
| `title` | string | Shown in the row's action hints. |
| `key`   | string | Optional extra shortcut. |
| `run`   | object | What to do; exactly one of the forms below. |
| `after` | string | `"close"` (default) or `"refresh"`. Ignored for `plugin` actions, which decide in their `result`. |

`run` forms. The core runs all of them through `Platform`, except `plugin`:

| form | effect |
|------|--------|
| `{"exec": [argv], "cwd": "..."}` | Start detached. |
| `{"terminal": [argv], "cwd": "..."}` | Start in the user's terminal. |
| `{"open": "url or path"}` | Open with the default handler. |
| `{"copy": "text"}` | Put text on the clipboard. |
| `{"plugin": "<action id>"}` | Send `activate` to the plugin and apply its `result`. |

`cwd` is optional and defaults to the home folder. Relative argv paths in
`exec` resolve against the plugin folder. argv is never run through a shell;
a plugin that wants shell features (the terminal plugin) puts the shell in
its own argv. Plugins describe *what* to run, never *how* on a given OS, so
they work unchanged on any platform the core supports.

### Icons

A string is a glyph (Nerd Font or emoji) when it is at most two grapheme
clusters long, otherwise a freedesktop icon name. An object is explicit:
`{"glyph": "󰐃"}`, `{"name": "folder"}` or `{"path": "/abs/icon.svg"}`.

### Filtering and ranking

For `list` plugins the core filters on `label`, then `detail`, then
`keywords`. A label match always outranks a detail or keyword match. Ties
keep the plugin's order, so a plugin controls the order of an empty query
(pinned first, then most recent). For `query` plugins the core shows items as
given, sorted by `score` when present.

### Environment

The core sets, for every plugin process:

| variable              | value |
|-----------------------|-------|
| `QL_PROTOCOL`         | `1` |
| `QL_PLUGIN_ID`        | the manifest id |
| `QL_PLUGIN_DIR`       | the plugin folder (also the working directory) |
| `QL_CONFIG_DIR`       | this plugin's config folder; created on first use |
| `QL_STATE_DIR`        | this plugin's state folder; created on first use |

On Omarchy the last two are `$XDG_CONFIG_HOME/quick-launcher/<id>` and
`$XDG_STATE_HOME/quick-launcher/<id>`. Plugins SHOULD use these variables
rather than building paths themselves, so they stay portable.

With `env = "session"` the plugin inherits the core's environment, which is
the fast path. With `env = "login"` the core starts it through the platform's
login shell (`bash -lc` on Omarchy, about 15 ms per start). That's for
plugins that read variables set only in shell profiles (pi's
`PI_CODING_AGENT_*`).

### Errors and timeouts

- Invalid manifest: the plugin is skipped and the reason is logged.
- No response within `timeout_ms`: the state's `error` says the plugin didn't
  respond, and a `oneshot` process is killed. A `persistent` process gets a
  second chance on the next request.
- A `persistent` plugin that exits is restarted on its next request, at most
  three times a minute. After that the core reports its error until the core
  restarts.
- An unparsable response line is logged and treated as an error response.
- Items missing `id`, `label` or `actions` are dropped and logged; the rest
  of the response is used.

## Platform

Everything OS-specific in the core sits behind one trait. Its only
implementation today is `Omarchy`:

```rust
pub trait Platform {
    /// Start argv detached from the core.
    fn spawn(&self, argv: &[String], cwd: Option<&Path>) -> io::Result<()>;
    /// Start argv in the user's terminal.
    fn terminal(&self, argv: &[String], cwd: Option<&Path>) -> io::Result<()>;
    /// Open a URL or path with its default handler.
    fn open(&self, target: &str) -> io::Result<()>;
    /// Put text on the clipboard.
    fn copy(&self, text: &str) -> io::Result<()>;
    /// Wrap argv so it runs with the user's login environment.
    fn login_command(&self, argv: &[String]) -> Vec<String>;
    /// Where bundled plugins, user plugins and config, and state live.
    fn dirs(&self) -> &Dirs;
}
```

| method          | Omarchy |
|-----------------|---------|
| `spawn`         | `setsid uwsm-app -- <argv>` |
| `terminal`      | `setsid uwsm-app -- xdg-terminal-exec --dir=<cwd> <argv>` |
| `open`          | `xdg-open` |
| `copy`          | `wl-copy` |
| `login_command` | `bash -lc 'exec "$@"' bash <argv>` |
| `dirs`          | XDG base directories, as listed above |

Core tests use a recording `Platform` that stores calls instead of running
them, so actions are tested without launching anything.

## Repository layout (non-normative)

```
Cargo.toml                 workspace
crates/core/               quick-launcher-core: library + binary
crates/plugin/             quick-launcher-plugin: SDK for Rust plugins
plugins/pi/                bundled plugin: plugin.toml + Rust binary
plugins/terminal/          bundled plugin: plugin.toml + Rust binary
ui/omarchy/                Omarchy shell plugin: manifest.json, Launcher.qml
docs/protocol.md           this document
install.py                 Omarchy installer: builds, copies, enables
```

## Testing

- The core's unit tests cover prefix parsing, hints, matching, ranking,
  action dispatch (through the recording `Platform`) and plugin process
  handling (through small test plugins).
- An end-to-end test drives the real core binary over the UI protocol from a
  script: hello, input, activate, close, with fixture plugins. It needs no
  Omarchy, Hyprland or Quickshell, which is the check that the core stays
  agnostic.
- The QML UI is checked with `qmllint` and by hand on Omarchy.

## Rust plugin SDK (non-normative)

A `quick-launcher-plugin` crate in this repo handles the transport, so a
plugin is mostly its own logic:

```rust
use quick_launcher_plugin::{Item, Action, Run, Activate, Outcome, Plugin};

struct Pi;

impl Plugin for Pi {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        Ok(projects()?.into_iter().map(|path| {
            Item::new(&path, name(&path))
                .detail(tilde(&path))
                .action(Action::new("open", "Open pi").run(Run::terminal(["pi"]).cwd(&path)))
                .action(Action::new("pin", "Pin").plugin().key("ctrl+p"))
        }).collect())
    }

    fn activate(&mut self, a: Activate) -> anyhow::Result<Outcome> {
        toggle_pin(&a.item)?;
        Ok(Outcome::refresh())
    }
}

fn main() { quick_launcher_plugin::run(Pi) }
```

`run` reads requests until EOF, so the same binary works as `oneshot` or
`persistent`. The crate also exports the core's matcher, so a `query` plugin
ranks the same way the core does.

## Examples

A complete plugin in shell, listing git repos under `~/Projects`:

```toml
# ~/.config/quick-launcher/plugins/repos/plugin.toml
protocol = 1
id = "repos"
name = "Repos"
icon = ""
prefixes = ["repo:"]
exec = ["./list.sh"]
```

```sh
#!/bin/sh
# list.sh: oneshot list plugin; ignores stdin.
find ~/Projects -maxdepth 2 -name .git -printf '%h\n' | jq -Rnc '{items: [inputs | {
  id: ., label: (split("/") | last), detail: .,
  actions: [{title: "Open in VS Code", run: {exec: ["code", .]}}]
}]}'
```

The bundled plugins, once ported:

| plugin     | kind  | lifetime   | notes |
|------------|-------|------------|-------|
| `pi`       | list  | oneshot    | `env = "login"`; pin/unpin via `plugin` actions |
| `terminal` | list  | oneshot    | `query_item` runs the typed command; history items run it again, Shift+Enter forgets it |

## Migrating from v0

| v0 (`modes.json` + `{"rows": [...]}`) | v1 |
|---|---|
| modes run by `Launcher.qml` | plugins run by the core |
| matching in `LauncherModel.js` | matching in the core |
| one `modes.json` entry per mode | one `plugin.toml` per plugin folder |
| `list` argv | `exec` + `kind = "list"`, request on stdin |
| row `action` | `actions[0]` with `run.exec` |
| row `secondary` | `actions[1]`, usually `run.plugin` with `after = "refresh"` |
| `query` row template | `[query_item]` |
| every command runs through `bash -lc` | only with `env = "login"` |

The switch happens in one step, not alongside v0. Running both would mean
keeping the v0 loader and matcher in QML next to the core, which is exactly
the logic the core exists to remove. Order of work:

1. The core and SDK, tested with fixture plugins and the scripted UI test.
2. `pi` and `terminal` ported to Rust plugins.
3. `Launcher.qml` switched to the UI protocol. `modes.json`, `modes/*.py` and
   `LauncherModel.js` are deleted in the same change.

## Open questions

1. **Global search.** Should a plugin be able to show items in the plain
   search (`global = true`), for example pinned pi projects next to apps?
   The UI protocol already supports it through `placement`.
2. **Prefix style.** `pi:` and `>` coexist today. Should prefixes follow one
   convention (`name:`), with symbols only as aliases?
3. **Distribution.** Bundled plugins and the core are built by `cargo build`
   at install time. Is requiring a Rust toolchain to install acceptable, or
   should releases include prebuilt binaries?
