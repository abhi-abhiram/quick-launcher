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
- **The core doesn't care about the frontend.** Any program that speaks the
  [frontend protocol](#frontend-protocol) is a frontend: the Omarchy shell
  window today, and a macOS window, a terminal client or a test script later.
  The core never knows which one is connected.
- **The core is platform-agnostic.** Omarchy is the only supported platform
  for now, but only one platform module inside the core knows that.
- **Rust first.** The core, the bundled plugins and the plugin SDK are Rust.
  Plugins only need to speak JSON over stdio, so a shell or Python plugin
  still works.

## Architecture

```
┌──────────────┐    frontend    ┌────────────────────┐  plugin protocol  ┌───────────────┐
│ frontend     │◄──────────────►│ core               │◄─────────────────►│ apps          │
│ (any; Omarchy│    protocol    │ (Rust)             │  JSON over stdio, ├───────────────┤
│ QML today)   │  JSON, stdio   │ plugins, prefixes, │  one process per  │ pi            │
│ draws state, │                │ ranking, actions   │  plugin           ├───────────────┤
│ sends input  │                │ ┌────────────────┐ │                   │ terminal      │
└──────────────┘                │ │ Platform:      │ │                   ├───────────────┤
                                │ │ Omarchy        │ │                   │ user plugins… │
                                │ └────────────────┘ │                   └───────────────┘
                                └────────────────────┘
```

- **Frontend**: draws the list and takes input. It holds no launcher logic
  and no search of its own: it sends what was typed or chosen and draws the
  state the core sends back. The only frontend today is the Omarchy shell
  plugin (`Launcher.qml`).
- **Core**: `quick-launcher-core`, one Rust binary. It discovers plugins,
  parses prefixes, runs plugin processes, filters and ranks every item (apps
  included), and runs actions through its platform module.
- **Platform**: a module inside the core holding everything OS-specific. See
  [Platform](#platform).
- **Plugin**: a folder with a `plugin.toml` manifest and an executable that
  speaks the [plugin protocol](#plugin-protocol).

### Apps are a plugin

Searching apps is a bundled plugin, `apps`, not frontend code. It reads the
platform's installed apps (desktop entries on Omarchy) and is
[`global`](#kinds), so its items show in the plain search. The core ranks apps
together with mode hints and other global items and sends one finished list.
Every frontend gets apps without writing an app search, and the core holds
everything that can be launched in one place.

Finding apps is OS-specific, and that code lives in the plugin: a macOS build
of it would read `/Applications` instead. Launching goes through `Platform`
with the [`app`](#actions) run form. The cost is Omarchy's app-library extras
(launch feedback, uninstall); see [Open questions](#open-questions).

### Rules that keep the core agnostic

- The core MUST NOT know which frontend is connected or behave differently
  for one. Nothing in the core refers to windows, keys, pointers, focus or
  layout.
- Everything a frontend shows arrives as data in the frontend protocol. Key
  bindings, layout, animation and icon rendering live only in the frontend.
- The core MUST NOT depend on Quickshell, Hyprland, Omarchy or any UI
  toolkit. Its crate builds and its tests pass on a machine without them.
- Every OS call (starting processes detached, terminals, opening URLs, the
  clipboard, config and state paths, login environments) goes through the
  `Platform` trait. No other core module calls OS tools directly.
- Nothing plugin-specific lives in the core. The core never knows what pi is.
- There is exactly one `Platform` implementation and one frontend until a
  second is actually being built. No stubs, no `cfg` branches for platforms
  that don't exist yet.

## Frontend protocol

A frontend starts the core as a long-running child process and keeps it for
as long as it wants results. The Omarchy shell plugin is `keepLoaded`, so it
keeps the core for the life of the shell; a one-shot terminal client could
start it per run. They exchange JSON Lines over the core's stdin and stdout;
the core's stderr is the log. Any program that can start a process and read
and write lines of JSON can be a frontend. Every message has `v` (protocol
version) and `type`.

Messages describe intent, not input devices. The Omarchy frontend's bindings
are given in parentheses as examples; another frontend picks its own.

### Frontend → core

| type         | fields | meaning |
|--------------|--------|---------|
| `hello`      | `client` (string), `version` (string), `max_items` (int, optional, default 50) | First message. `client` is for logs only; the core MUST NOT behave differently because of it. `max_items` caps `items` in every `state`. The core replies `ready`. |
| `open`       | `mode` (optional plugin id) | A session starts (window shown), optionally straight into a mode. The core resets to that mode (or none) with empty text, refreshes list plugins as needed, and sends `state`. |
| `input`      | `seq` (int), `text` (string) | The typed text changed. `text` is all of it, which in a mode excludes the prefix. `seq` increases with every `input`. |
| `leave_mode` | — | Leave the active mode. The core puts the mode's prefix, minus its last character, back into the text, as if the user had deleted the `:`. (Backspace on empty text.) |
| `complete`   | — | Enter the mode of the first mode hint, if any. (Tab.) |
| `activate`   | `item` (string), `action` (string) | Run an action. `item` is the item's `ref`, `action` an action `id`. (Enter, Shift+Enter, click, an action's `key`.) |
| `close`      | — | The session ended without running anything. (Escape, window hidden.) |

Closing stdin ends the core.

### Core → frontend

| type    | fields | meaning |
|---------|--------|---------|
| `ready` | `protocol` (int), `version` (string) | Reply to `hello`. |
| `state` | see below | The complete state to draw. Sent after every message that changes it and whenever a plugin answers. |
| `done`  | — | An action ran and the session is over. A windowed frontend hides itself; a one-shot client exits. |

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
| `text`    | The typed text. Differs from what the frontend sent when the core changed mode, e.g. typing `pi:launch` gives mode `pi` and text `launch`. |
| `loading` | A plugin request for this mode and text is still outstanding. |
| `error`   | A one-line, user-readable message, or `null`. |
| `items`   | Items to show, best first, at most `max_items`. This is the whole list, apps and mode hints included; the frontend adds nothing to it. |

**Keeping the text in sync.** A user can type faster than the core answers.
A frontend MUST NOT copy `text` into its input from a `state` whose `seq` is
lower than the last `seq` it sent. It still draws that state's items, which
the core recomputes for its own newest text anyway.

### Frontend items

What a frontend needs to draw a row, with no knowledge of how actions run:

```json
{"ref":"pi/~/Projects/quick-launcher","label":"quick-launcher",
 "detail":"~/Projects/quick-launcher","icon":{"glyph":"π"},"badge":"pinned",
 "disabled":false,
 "actions":[{"id":"open","title":"Open pi"},{"id":"unpin","title":"Unpin","key":"ctrl+p"}]}
```

- `ref` identifies the item to the core; the frontend treats it as opaque.
- `icon` is always an explicit object (`glyph`, `name` or `path`); the core
  has already resolved the shorthand forms plugins may use. A frontend that
  can't draw a form shows no icon.
- The first action is the item's **default**, the second its **alternate**.
  A frontend SHOULD make both easy to reach; how is up to it. `key` is a
  suggested shortcut that a frontend MAY bind and MAY ignore. The Omarchy
  frontend runs the default on Enter or click, the alternate on Shift+Enter
  or right-click, binds every `key`, and shows the selected row's other
  actions with their keys.

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
global = false                    # list plugins only: also searched in the
                                  #   plain search; see Kinds
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

  A list plugin with `global = true` is also asked for its items when the
  launcher opens, and the core searches them in the plain search, not only in
  the plugin's mode. `apps` is global.
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

The core turns each item into a [frontend item](#frontend-items), with `ref`
built from the plugin id and the item `id`.

### Actions

The first action is the item's default and the second its alternate. How a
user triggers them is up to the frontend (see
[Frontend items](#frontend-items)).

| field   | type   | meaning |
|---------|--------|---------|
| `id`    | string | Sent back in `activate`. Defaults to the action's index. |
| `title` | string | Names the action wherever the frontend lists actions. |
| `key`   | string | Optional suggested shortcut, e.g. `"ctrl+d"`. Frontends MAY bind it. |
| `run`   | object | What to do; exactly one of the forms below. |
| `after` | string | `"close"` (default) or `"refresh"`. Ignored for `plugin` actions, which decide in their `result`. |

`run` forms. The core runs all of them through `Platform`, except `plugin`:

| form | effect |
|------|--------|
| `{"exec": [argv], "cwd": "..."}` | Start detached. |
| `{"terminal": [argv], "cwd": "..."}` | Start in the user's terminal. |
| `{"app": "<app id>"}` | Launch an installed app by its platform id (a desktop entry id such as `firefox.desktop` on Omarchy). Used by `apps`. |
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

The plain search runs the same matcher over every global plugin's items and
the mode hints, and sorts them all together:

1. Mode hints whose text names the mode outright (`pi`, `term`).
2. Global items, by match quality. Ties go to plugins in discovery order,
   then each plugin's own order. With empty text, each plugin's own order.
3. Other mode hints.

This keeps today's order (named modes, then apps, then other modes) while
leaving room for more global plugins.

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
    /// Launch an installed app by its platform id.
    fn launch_app(&self, id: &str) -> io::Result<()>;
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
| `launch_app`    | `setsid uwsm-app -- <id>` |
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
plugins/apps/              bundled plugin: plugin.toml + Rust binary
plugins/pi/                bundled plugin: plugin.toml + Rust binary
plugins/terminal/          bundled plugin: plugin.toml + Rust binary
frontends/omarchy/         Omarchy shell plugin: manifest.json, Launcher.qml
docs/protocol.md           this document
install.py                 Omarchy installer: builds, copies, enables
```

## Testing

- The core's unit tests cover prefix parsing, hints, matching, ranking,
  action dispatch (through the recording `Platform`) and plugin process
  handling (through small test plugins).
- An end-to-end test drives the real core binary over the frontend protocol
  from a script: hello, input, activate, close, with fixture plugins. The
  script is a second frontend that needs no Omarchy, Hyprland or Quickshell,
  which is the check that the core assumes nothing about the Omarchy one.
- The QML frontend is checked with `qmllint` and by hand on Omarchy.

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
| `apps`     | list  | oneshot    | `global = true`; reads desktop entries, skips `NoDisplay`/`Hidden`, launches with `run.app` |
| `pi`       | list  | oneshot    | `env = "login"`; pin/unpin via `plugin` actions |
| `terminal` | list  | oneshot    | `query_item` runs the typed command; history items run it again, Shift+Enter forgets it |

## Migrating from v0

| v0 (`modes.json` + `{"rows": [...]}`) | v1 |
|---|---|
| modes run by `Launcher.qml` | plugins run by the core |
| matching in `LauncherModel.js` | matching in the core |
| apps searched by `Launcher.qml` through Omarchy's app library | bundled `apps` plugin, ranked by the core |
| one `modes.json` entry per mode | one `plugin.toml` per plugin folder |
| `list` argv | `exec` + `kind = "list"`, request on stdin |
| row `action` | `actions[0]` with `run.exec` |
| row `secondary` | `actions[1]`, usually `run.plugin` with `after = "refresh"` |
| `query` row template | `[query_item]` |
| every command runs through `bash -lc` | only with `env = "login"` |

The switch happens in one step, not alongside v0. Running both would mean
keeping the v0 loader and matcher in QML next to the core, which is exactly
the logic the core exists to remove. Order of work:

1. The core and SDK, tested with fixture plugins and the scripted frontend test.
2. `apps`, `pi` and `terminal` written as Rust plugins.
3. `Launcher.qml` switched to the frontend protocol, with its app search
   removed. `modes.json`, `modes/*.py` and `LauncherModel.js` are deleted in
   the same change.

## Open questions

1. **Omarchy app extras.** Launch feedback and uninstall came from Omarchy's
   app library. Are they worth adding back, as `apps` actions or `Platform`
   methods?
2. **Prefix style.** `pi:` and `>` coexist today. Should prefixes follow one
   convention (`name:`), with symbols only as aliases?
3. **Distribution.** Bundled plugins and the core are built by `cargo build`
   at install time. Is requiring a Rust toolchain to install acceptable, or
   should releases include prebuilt binaries?
