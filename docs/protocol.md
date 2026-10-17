# Quick launcher architecture and protocols (v1, draft)

Status: **draft for review**. Nothing here is implemented yet. It replaces the
`modes.json` contract described in the README ("v0"); the switch happens in
one step, see [Migrating from v0](#migrating-from-v0).

The keywords MUST, SHOULD and MAY are used as in RFC 2119.

## Goals

- **Adding a plugin is dropping in a folder.** A manifest plus one Rust
  binary built with the plugin SDK. No core changes, no core rebuild, no
  shell restart.
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
- **Rust only, for the core and every plugin.** Plugins are written against
  the SDK's `Plugin` trait. How the SDK talks to the core is private to the
  two, so it can change without breaking plugin code. This is a contract,
  not a barrier: the wire format is plain JSON that a Python script could
  speak. Ruling that out gives up a plugin ecosystem open to every language
  in exchange for being free to change the wire format in any release; such
  plugins are unsupported and may break on any update. Frontends are the
  exception: they use whatever their toolkit needs (QML today), which is why
  the frontend protocol is public JSON.

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
- **Plugin**: a folder with a `plugin.toml` manifest and a Rust binary built
  with the [plugin SDK](#plugin-sdk), running as its own process.

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
the core's stderr is the log. Any program, in any language, that can start a
process and read and write lines of JSON can be a frontend. Unlike the
plugin wire format, this protocol is public and stable. Every message has
`v`, the frontend protocol's major version, and `type`. No other field
carries the protocol version.

Messages describe intent, not input devices. The Omarchy frontend's bindings
are given in parentheses as examples; another frontend picks its own.

### Frontend → core

| type         | fields | meaning |
|--------------|--------|---------|
| `hello`      | `client` (string), `client_version` (string), `max_items` (int, optional, default 50) | First message; its `v` is the major the frontend speaks. `client` and `client_version` name the frontend build, for logs only; the core MUST NOT behave differently because of them. `max_items` caps `items` in every `state`. The core replies `ready`. |
| `open`       | `mode` (optional plugin id) | A session starts (window shown), optionally straight into a mode. The core resets to that mode (or none) with empty text, refreshes list plugins as needed, and sends `state`. |
| `input`      | `seq` (int), `mode` (plugin id or `null`), `text` (string) | The typed text changed. `mode` is the mode the frontend is showing with its text (`null` before the first `state`), and `text` is all the text typed in that mode, without its prefix. `seq` increases with every `input`. See [Reading input as the user saw it](#reading-input-as-the-user-saw-it). |
| `leave_mode` | — | Leave the active mode. The core puts the prefix the user typed to enter it (the mode's first prefix if it was entered another way), minus its last character, back into the text, as if the user had deleted the `:`. Does nothing without an active mode. (Backspace on empty text.) |
| `complete`   | `item` (string, optional) | Enter a mode: the selected row's, when `item` is a [mode hint](#mode-hints)'s `ref`, otherwise the first mode hint's, if any. (Tab.) |
| `activate`   | `item` (string), `action` (string) | Run an action. `item` is the item's `ref`, `action` an action `id`. (Enter, Shift+Enter, click, an action's `key`.) |
| `close`      | — | The session ended without running anything. (Escape, window hidden.) |

Closing stdin ends the core.

If an `activate` names a `ref` the core can no longer resolve (a newer
`state` dropped that item before the user's choice arrived), the core logs
it, runs nothing and sends the current `state` again.

### Core → frontend

| type    | fields | meaning |
|---------|--------|---------|
| `ready` | `core_version` (string) | Reply to every `hello`, even one whose version the core can't speak. Its `v` is the core's major; `core_version` names the core build. |
| `state` | see below | The complete state to draw. Sent after every message that changes it and whenever a plugin answers. |
| `done`  | — | An action ran and the session is over. A windowed frontend hides itself; a one-shot client exits. |

**Versions.** A core speaks one major. If a `hello`'s `v` differs from it,
the core still sends `ready` (carrying its own `v`), then logs why and exits
with a non-zero status, so the frontend learns the core's major before it
sees EOF. A frontend that gets a `ready` whose `v` differs from its own MUST
stop and report it (on stderr, or as a notification) rather than guess.
Within a major, both sides MUST ignore message types and fields they don't
know.

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
| `error`   | A one-line, user-readable message, or `null`. Drawn alongside `items`, never instead of them. |
| `items`   | Items to show, best first, at most `max_items`. This is the whole list, apps and mode hints included; the frontend adds nothing to it. |

### Reading input as the user saw it

A user can type faster than the core answers, so the frontend's view and the
core's can briefly differ. Two rules keep them consistent:

- **The core reads `input` in the frontend's mode.** If `input.mode` differs
  from the core's mode, the core switches to `input.mode` first, then parses
  `text` there. Typing `pi:` then `pi:l` before any reply sends both with
  `mode: null`; both enter pi mode, and the second leaves text `l`. Typing
  right after `complete` or `leave_mode` works the same way: what the user
  typed is read against the screen they typed it on.
- **Mode and text change together, and never from a stale state.** A
  `state` whose `seq` is lower than the last `seq` the frontend sent is
  stale. The frontend draws its `items`, `loading` and `error`, but keeps
  its own mode and text: it MUST NOT take either from a stale state, and it
  takes both from a current one. The items are only briefly out of date,
  since the core recomputes them for its own newest text anyway. A `state`
  answering `leave_mode` or `complete` carries the last `seq` sent, so it
  is current unless the user typed after it.

  Taking the mode alone would break this. Sending `pi:` then `pi:l`, drawing
  the first reply's mode `pi` while keeping the stale text `pi:l`, and then
  typing `a` would send `mode: pi, text: "pi:la"`, reading the prefix as
  search text. With the rule, the frontend still shows mode `null` and sends
  `mode: null, text: "pi:la"`, which the core parses as mode `pi`, text
  `la`.

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

### Mode hints

In the plain search the core adds an item for each mode whose name or
prefix matches the text (ranked as in
[Filtering and ranking](#filtering-and-ranking)). It's an ordinary item to
the frontend, with one action, `{"id": "enter", "title": "Open <name>"}`.
Activating it enters that mode with empty text, as `complete` would, and
sends `state` rather than `done`. So a frontend that draws items generically
can enter any mode by selection, with no special case for hints.

## Plugins

Plugins are Rust only. A plugin is a folder holding a `plugin.toml` manifest
and one binary built with the `quick-launcher-plugin` crate. That crate's
types and its `Plugin` trait are the plugin contract, together with the
manifest and the item, action and environment rules below.

Each plugin still runs as its own process, so a plugin that panics, hangs or
leaks can't take the core down; the core times it out or restarts it. How
the SDK and the core talk is the [wire format](#wire-format-internal), which
is private to the two of them.

### Plugin SDK

The SDK handles the process side (reading requests, writing responses, the
environment), so a plugin is only its own logic:

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
        if let Some(item) = &a.item {
            toggle_pin(item)?;
        }
        Ok(Outcome::refresh())
    }
}

fn main() { quick_launcher_plugin::run(Pi) }
```

- `run` reads requests until EOF, so the same binary works as `oneshot` or
  `persistent`.
- `Activate::item` is `None` for the `[query_item]`, whose actions use
  `Activate::text` instead.
- Methods a plugin doesn't need (`query` for a list plugin, `activate` for
  one without `plugin` actions) have defaults that return an error.
- The crate exports the core's matcher, so a `query` plugin ranks the same
  way the core does.
- Panics are caught and turned into an error response, so a bug in a
  persistent plugin costs one request, not a restart.
- `query` gets a handle that can send partial results before the final
  list, so slow answers (an AI reply, a network search) stream in, and that
  reports when the core has cancelled the request because the text changed.
- A plugin is built with plain `cargo build --release`; the binary and
  `plugin.toml` go in the plugin folder.

### Plugin discovery

The core reads plugins from, in order:

1. the bundled plugin directory, `$XDG_DATA_HOME/quick-launcher/plugins/<id>/`
2. the user plugin directory, `$XDG_CONFIG_HOME/quick-launcher/plugins/<id>/`

These are the Omarchy (Linux) paths; the core gets them from `Platform`. The
bundled directory is per user because `install.py` builds the plugins from
source for that user. If releases ship prebuilt binaries instead (see
Distribution in [Open questions](#open-questions)), bundled plugins move to
the install prefix, so the two decisions change together. A
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
protocol = 1                      # required; wire format major of the SDK
                                  #   the binary was built with
id = "pi"                         # required; [a-z0-9-]+, unique
name = "pi"                       # required; shown in hints and the mode chip
description = "Open pi in a project"
icon = "π"                        # see Icons
prefixes = ["pi:"]                # required; at least one, matched case-insensitively
placeholder = "Open pi in…"       # shown in the empty search field
bin = "pi-plugin"                 # required; the plugin's binary, in the
                                  #   plugin folder
kind = "list"                     # "list" (default) or "query"
global = false                    # list plugins only: also searched in the
                                  #   plain search; see Kinds
lifetime = "oneshot"              # "oneshot" (default) or "persistent"
env = "session"                   # "session" (default) or "login"; see Environment
timeout_ms = 2000                 # per request; default 2000

# Optional: an item built from the typed text, shown first. Only for
# kind = "list". `{query}` is replaced in every string, including argv,
# with the typed text trimmed. Not shown when that text is empty, or when
# an item still in the filtered list has exactly that label (so a command
# already in terminal history appears once).
# It has no `id`: its `plugin` actions are sent with `item` set to null
# and the typed `text`.
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

- **`oneshot`**: the core starts the binary for one request, takes its
  response and reaps it (see [Transport](#transport) for the exact steps). Best when the plugin's work is quick and its data changes between
  opens.
- **`persistent`**: the core starts the binary on first use and keeps it
  running for later requests, so it can keep state and caches in memory.
  When the core exits the plugin is told to stop, and it MUST exit.

The SDK handles both; the lifetime is only a manifest setting.

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
built from the plugin id and the item `id`. The `[query_item]` gets a `ref`
of its own that no listed item can have, so it never collides with one.
That `ref` also records the text the item was built from. Activating it
runs the actions filled in with that text, which is what the user saw, even
if newer input has changed the text in the meantime.

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

A string is always a glyph (Nerd Font or emoji), however long. Icon names
and files use the object form: `{"name": "folder"}` (a freedesktop icon
name on Omarchy) or `{"path": "/abs/icon.svg"}`. `{"glyph": "󰐃"}` is the
long form of a string. There's no guessing from length, so a short icon
name or a glyph made of several code points can't be misread.

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
(the SDK exposes them as `quick_launcher_plugin::dirs()`) rather than building
paths themselves, so they stay portable.

With `env = "session"` the plugin inherits the core's environment, which is
the fast path. With `env = "login"` the core starts it through the platform's
login shell (`bash -lc` on Omarchy, about 15 ms per start). That's for
plugins that read variables set only in shell profiles (pi's
`PI_CODING_AGENT_*`).

The setting also covers the plugin's actions. The core runs `exec` and
`terminal` actions from a `login` plugin's items through
`Platform::login_command` too, so a command found on the profile's `PATH`
while listing is also found when launched, with the same variables. `open`,
`copy` and `app` run no plugin command and are unaffected. `[query_item]`
actions follow the same rule.

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

### Wire format (internal)

This is how the SDK and the core talk, written down for whoever maintains the
two. Plugins MUST NOT implement it themselves. The core and the SDK share
their message types through one internal crate, so they can't drift apart,
and they MAY change the format together in any release.

It's JSON Lines because a plugin can be debugged by piping a request into it
(`echo '{"v":1,"id":1,"method":"list","params":{}}' | ./pi-plugin`). Encoding
cost doesn't matter here: a `list` response with a few thousand items parses
in about a millisecond and is read once per mode entry, and a `query`
response is a few KB. Starting the process and the plugin's own work cost
far more. If JSON ever shows up in a profile, the encoding changes inside
the core and the SDK, and plugins only need a rebuild.

**Compatibility.** `protocol` in the manifest and `v` in every message are
the wire format's major version. A plugin whose major differs from the
core's is skipped, and the log asks for it to be rebuilt against the current
SDK. Within a major, unknown fields MUST be ignored, so a plugin built with
an older SDK keeps working.

#### Transport

- JSON Lines over stdio: one UTF-8 JSON object per line, `\n` terminated.
- stdin carries requests, stdout carries responses. The SDK writes nothing
  else to stdout, so plugins MUST NOT print to stdout themselves.
- stderr is for logs. The core forwards it to its own log, prefixed with the
  plugin id.
- A line MUST NOT exceed 4 MiB.
- **`oneshot`**: the core writes the request line and then closes the
  plugin's stdin. The SDK answers, reads EOF and exits. The core uses each
  response line as soon as it arrives, without waiting for the exit. Once
  the final response (one without `more`) has arrived, the core reaps the
  process, killing it if it hasn't exited within `timeout_ms`. Partial
  responses don't start that deadline, so a one-shot plugin can stream
  under the same between-responses timeout as a persistent one.
- **`persistent`**: stdin stays open between requests. When the core exits
  it closes every persistent plugin's stdin, and the SDK exits at EOF.

#### Requests

Every request has `v` (protocol version), `id` (an integer, unique per plugin
process) and `method`.

| method     | sent to                    | params                         |
|------------|----------------------------|--------------------------------|
| `list`     | `list` plugins             | `{}`                           |
| `query`    | `query` plugins            | `{"text": "what was typed"}`   |
| `activate` | any plugin, for a `plugin` action | `{"item": "<item id>" or null, "action": "<action id>", "text": "..."}`; `item` is `null` for the `[query_item]` |
| `cancel`   | `persistent` plugins       | `{"request": <id>}`: stop work on that request; no response |

```json
{"v":1,"id":7,"method":"query","params":{"text":"2*21"}}
```

`text` is what follows the prefix, with leading whitespace removed.

#### Responses

Every response has `v` and echoes the request `id`.

```json
{"v":1,"id":7,"items":[ ... ]}
{"v":1,"id":8,"result":{"close":true}}
{"v":1,"id":9,"error":{"message":"pinned.json is not valid JSON"}}
```

- `list` and `query` return `items`.
- `activate` returns `result`:
  - `close` (bool, default `true`): close the launcher.
  - `refresh` (bool, default `false`): re-request the mode's items. Implies
    the launcher stays open.
  - `query` (string, optional): replace the typed text.
- `error` becomes the state's `error`. If it answers a `list` or `query`,
  that plugin contributes no items for that request. An error from
  `activate`, or a timeout after a partial answer, leaves the items already
  shown in place. The message SHOULD be one short, user-readable sentence.

A `query` response MAY set `"more": true`. It's a partial answer: the core
replaces that request's items with it, sends a `state` with `loading: true`,
and keeps waiting. The response without `more` is final. `timeout_ms`
applies to the gap before each response, not to the whole stream.

For a `persistent` plugin the core may send a new `query` before the previous
one is answered. It uses only responses to the newest request and drops the
rest. It sends `cancel` for the superseded one (a superseded `oneshot`
process is killed instead). A plugin that ignores
`cancel` is still correct, only wasteful; one doing expensive work (calling
a model, say) SHOULD stop.

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
    /// Wrap argv so it runs with the user's login environment. Used for
    /// `login` plugins and for their `exec` and `terminal` actions.
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

## Designed for later: AI

AI integration isn't part of v1, but v1 is shaped so it can be added
without breaking frontends or plugins:

- **One catalog.** Everything launchable (apps, projects, commands) is an
  item the core already holds, in one shape. An AI feature gets the whole
  catalog from the core, never from a frontend.
- **Actions are data.** An AI can only pick items and actions that plugins
  offered, and the core runs them exactly as if the user had chosen them,
  through `Platform` and never through a shell.
- **Slow answers fit.** Partial `query` responses, `cancel` and per-plugin
  `timeout_ms` let an AI plugin stream a reply, stop when the text changes,
  and take longer than a list plugin.

Expected additions, as new optional fields and requests that older
frontends and plugins ignore:

- A plugin request that reads the other plugins' items and actions, so an
  AI plugin can see the catalog. Off unless the user grants it per plugin in
  `config.toml`.
- A way for a plugin to propose actions that run only after the user
  confirms them in the frontend.
- An optional long-text field on items for multi-line answers, drawn by
  frontends that support it.

## Repository layout (non-normative)

```
Cargo.toml                 workspace
crates/core/               quick-launcher-core: library + binary
crates/plugin/             quick-launcher-plugin: the plugin SDK
crates/wire/               wire format types shared by core and SDK (internal)
plugins/apps/              bundled plugin: plugin.toml + Rust binary
plugins/pi/                bundled plugin: plugin.toml + Rust binary
plugins/terminal/          bundled plugin: plugin.toml + Rust binary
frontends/omarchy/         Omarchy shell plugin: manifest.json, Launcher.qml
docs/protocol.md           this document
install.py                 Omarchy installer: builds, copies, enables
```

### Repository strategy

Everything stays in this one repository for now.

- **Core, SDK, wire crate and bundled plugins always live together.** The
  wire format is private and changes with the SDK in any release, so
  splitting them would turn every wire change into coordinated releases
  across repositories.
- **The frontend is the only clean cut.** It talks to the core only through
  the public frontend protocol, and after the v1 switch it is all that
  remains of today's repository (`Launcher.qml`, `manifest.json`,
  `install.py`).
- **Not yet, because the frontend protocol is still moving.** While it
  changes, most changes touch both sides, and `install.py` builds the core
  from source next to the QML. A separate frontend repository would first
  need a way to get the core (see Distribution in
  [Open questions](#open-questions)).

Split when one of these holds: the frontend protocol is tagged stable, a
second frontend is being built, or the core and SDK are published for use
without Omarchy. The split is then:

| repository | contents |
|------------|----------|
| `quick-launcher` | core, SDK, wire crate, bundled plugins, this document; publishes binaries and the SDK crate |
| `quick-launcher-omarchy` | Omarchy frontend and installer; installs a pinned core release |

Plugin authors are unaffected either way: they depend on the SDK crate,
wherever it lives.

## Testing

- The core's unit tests cover prefix parsing, hints, matching, ranking,
  action dispatch (through the recording `Platform`) and plugin process
  handling (through fixture plugins built with the SDK).
- The SDK's tests cover its request loop, panic handling and defaults
  against the shared wire types.
- An end-to-end test drives the real core binary over the frontend protocol
  from a script: hello, input, activate, close, with fixture plugins. The
  script is a second frontend that needs no Omarchy, Hyprland or Quickshell,
  which is the check that the core assumes nothing about the Omarchy one.
- The QML frontend is checked with `qmllint` and by hand on Omarchy.

## Examples

A complete plugin listing git repos under `~/Projects`:

```toml
# ~/.config/quick-launcher/plugins/repos/plugin.toml
protocol = 1
id = "repos"
name = "Repos"
icon = ""
prefixes = ["repo:"]
bin = "repos"
```

```rust
// src/main.rs; `cargo build --release`, then copy target/release/repos
// into the plugin folder.
use quick_launcher_plugin::{Action, Item, Plugin, Run};
use std::{env, fs, path::PathBuf};

struct Repos;

impl Plugin for Repos {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        let root = PathBuf::from(env::var("HOME")?).join("Projects");
        Ok(fs::read_dir(root)?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.join(".git").is_dir())
            .map(|path| {
                let dir = path.display().to_string();
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                Item::new(&dir, name)
                    .detail(&dir)
                    .action(Action::new("open", "Open in VS Code").run(Run::exec(["code", &dir])))
            })
            .collect())
    }
}

fn main() { quick_launcher_plugin::run(Repos) }
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
| `list` argv, any language | Rust binary built with the SDK, `bin` + `kind = "list"` |
| row `action` | `actions[0]` with `run.exec` |
| row `secondary` | `actions[1]`, usually `run.plugin` with `after = "refresh"` |
| `query` row template, rebuilt by QML on each keystroke | `[query_item]`, filled in by the core with no process start |
| every command runs through `bash -lc` | only with `env = "login"` |

The switch happens in one step, not alongside v0. Running both would mean
keeping the v0 loader and matcher in QML next to the core, which is exactly
the logic the core exists to remove. Order of work:

1. The core and SDK, tested with fixture plugins and the scripted frontend test.
2. `apps`, `pi` and `terminal` written as Rust plugins.
3. `Launcher.qml` switched to the frontend protocol, with its app search
   removed. `modes.json`, `modes/*.py` and `LauncherModel.js` are deleted in
   the same change.

**User data.** v0 keeps pi pins and terminal history outside the per-plugin
folders:

| data | v0 | v1 |
|------|----|----|
| pi pins | `$XDG_CONFIG_HOME/quick-launcher/pi-pinned.json` | `QL_CONFIG_DIR` (`$XDG_CONFIG_HOME/quick-launcher/pi/`) |
| terminal history | `$XDG_STATE_HOME/quick-launcher/terminal-history.json` | `QL_STATE_DIR` (`$XDG_STATE_HOME/quick-launcher/terminal/`) |

Each plugin reads its v0 file while its own file doesn't exist yet, and
writes only to its own, the way `modes/pi.py` already reads pins left by the
older pi-launcher widget (`pi` keeps that fallback too, after the v0 file).
v0 files are never changed, so going back to v0 loses nothing.

## Open questions

1. **Omarchy app extras.** Launch feedback and uninstall came from Omarchy's
   app library. Are they worth adding back, as `apps` actions or `Platform`
   methods?
2. **Prefix style.** `pi:` and `>` coexist today. Should prefixes follow one
   convention (`name:`), with symbols only as aliases?
3. **Distribution.** Bundled plugins and the core are built by `cargo build`
   at install time. Writing a plugin needs a Rust toolchain anyway, but is
   it acceptable just to install, or should releases include prebuilt
   binaries?
