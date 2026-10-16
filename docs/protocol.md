# Plugin protocol (v1, draft)

Status: **draft for review**. Nothing here is implemented yet. It replaces the
`modes.json` contract described in the README ("v0"), which keeps working
until every bundled mode is ported; see [Migrating from v0](#migrating-from-v0).

The keywords MUST, SHOULD and MAY are used as in RFC 2119.

## Goals

- **Adding a plugin is dropping in a folder.** A manifest plus one executable,
  in any language. No launcher code changes, no rebuild, no shell restart.
- **Typing never waits on a process.** Most plugins list their items once and
  the launcher filters them. Plugins that must see every keystroke stay
  running, so there is no startup cost per keystroke.
- **Nothing passes through a shell.** Actions are argv arrays.
- **Rust first.** The bundled plugins and the SDK are Rust; the protocol is
  plain JSON over stdio, so a shell or Python plugin still works.

## Roles

- **Launcher**: the Omarchy shell plugin (`Launcher.qml`). It discovers
  plugins, parses prefixes, runs plugin processes, filters and ranks items,
  draws the UI and runs actions. Apps search stays built in, because it uses
  the shell's own app library (icons, hidden entries, launch feedback).
- **Plugin**: a folder with a `plugin.toml` manifest and an executable that
  speaks this protocol.

## Plugin discovery

The launcher reads plugins from, in order:

1. `<launcher install dir>/plugins/<id>/plugin.toml`: bundled plugins
2. `$XDG_CONFIG_HOME/quick-launcher/plugins/<id>/plugin.toml`: user plugins

A user plugin with the same `id` as a bundled one replaces it. The folder name
MUST equal the manifest `id`. Both directories are watched; adding, editing or
removing a plugin takes effect the next time the launcher opens.

User overrides live in `$XDG_CONFIG_HOME/quick-launcher/config.toml`:

```toml
disabled = ["terminal"]          # plugin ids to skip

[plugins.pi]
prefixes = ["p:", "pi:"]         # replaces the manifest's prefixes
```

If two enabled plugins claim the same prefix, the one discovered first keeps
it and the launcher logs a warning naming both.

## Manifest: `plugin.toml`

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

Unknown keys MUST be ignored, so newer manifests load in older launchers.

### Kinds

- **`list`**: the launcher sends one `list` request when the mode is entered
  (and again each time the launcher opens while in it). The plugin returns
  every item, and the launcher filters and ranks them as you type. Use this
  whenever the full set is small enough to send at once (up to a few
  thousand items). pi projects and terminal history are list plugins.
- **`query`**: the launcher sends a `query` request on every keystroke, and
  the plugin returns items for that text, already filtered and ranked. Use
  this when the items depend on the text (calculator, web search, file search
  over a large tree). Query plugins SHOULD be `persistent`.

### Lifetimes

- **`oneshot`**: the launcher starts the executable, writes one request line,
  closes stdin, reads one response line and waits for it to exit. A plugin
  that needs no input MAY ignore stdin entirely and just print its response.
- **`persistent`**: the launcher starts the executable on first use and keeps
  it running for the life of the shell, writing one request per line and
  reading one response per line. On launcher close it sends nothing; on shell
  exit it closes stdin, and the plugin MUST exit when stdin reaches EOF.

## Transport

- JSON Lines over stdio: one UTF-8 JSON object per line, `\n` terminated.
- stdin carries requests, stdout carries responses. A plugin MUST NOT write
  anything else to stdout.
- stderr is for logs. The launcher forwards it to the shell log, prefixed
  with the plugin id.
- A line MUST NOT exceed 4 MiB.

## Requests

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

## Responses

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
- `error` is shown as a single dimmed row in place of the items. The message
  SHOULD be one short, user-readable sentence.

For a `persistent` plugin the launcher may send a new `query` before the
previous one is answered. It uses only the response to the newest request and
drops the rest, so plugins MAY answer in order without tracking cancellation.

## Items

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

`run` forms, all handled by the launcher except `plugin`:

| form | effect |
|------|--------|
| `{"exec": [argv], "cwd": "..."}` | Start detached, as its own `uwsm-app` scope. |
| `{"terminal": [argv], "cwd": "..."}` | Start in the default terminal (`xdg-terminal-exec`). |
| `{"open": "url or path"}` | `xdg-open`. |
| `{"copy": "text"}` | Put text on the clipboard. |
| `{"plugin": "<action id>"}` | Send `activate` to the plugin and apply its `result`. |

`cwd` is optional and defaults to `$HOME`. Relative argv paths in `exec`
resolve against the plugin folder. argv is never run through a shell; a
plugin that wants shell features (the terminal plugin) puts the shell in its
own argv.

### Icons

A string is a glyph (Nerd Font or emoji) when it is at most two grapheme
clusters long, otherwise a freedesktop icon name. An object is explicit:
`{"glyph": "󰐃"}`, `{"name": "folder"}` or `{"path": "/abs/icon.svg"}`.

## Filtering and ranking

For `list` plugins the launcher filters on `label`, then `detail`, then
`keywords`. A label match always outranks a detail or keyword match. Ties
keep the plugin's order, so a plugin controls the order of an empty query
(pinned first, then most recent). For `query` plugins the launcher shows
items as given, sorted by `score` when present.

## Environment

The launcher sets, for every plugin process:

| variable              | value |
|-----------------------|-------|
| `QL_PROTOCOL`         | `1` |
| `QL_PLUGIN_ID`        | the manifest id |
| `QL_PLUGIN_DIR`       | the plugin folder (also the working directory) |
| `QL_CONFIG_DIR`       | `$XDG_CONFIG_HOME/quick-launcher/<id>`; created on first use |
| `QL_STATE_DIR`        | `$XDG_STATE_HOME/quick-launcher/<id>`; created on first use |

With `env = "session"` the plugin inherits the shell's environment, which is
the fast path. With `env = "login"` the launcher starts it through
`bash -lc`, adding about 15 ms per start. That's for plugins that read
variables set only in shell profiles (pi's `PI_CODING_AGENT_*`).

## Errors and timeouts

- Invalid manifest: the plugin is skipped and the reason is logged.
- No response within `timeout_ms`: the launcher shows "pi didn't respond" and
  kills a `oneshot` process. A `persistent` process gets a second chance on
  the next request.
- A `persistent` plugin that exits is restarted on its next request, at most
  three times a minute, after which the launcher shows its error row until
  the shell restarts.
- An unparsable response line is logged and treated as an error response.
- Items missing `id`, `label` or `actions` are dropped and logged; the rest
  of the response is used.

## Versioning

`protocol` in the manifest and `v` in each request carry the major version.
A launcher MUST skip plugins whose `protocol` is newer than it supports.
Additive changes (new optional fields, new `run` forms) do not bump the
version, so plugins MUST ignore unknown fields and launchers MUST ignore
unknown fields in responses.

## Security

Plugins run unsandboxed as the user, like Omarchy shell plugins. Only install
plugins whose code you trust. The protocol avoids adding risk of its own:
argv is never interpreted by a shell, and typed text reaches a plugin only as
JSON.

## Rust SDK (non-normative)

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
`persistent`. The crate also exports the launcher's matcher, so a `query`
plugin ranks the same way the launcher does.

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
| one `modes.json` entry per mode | one `plugin.toml` per plugin folder |
| `list` argv | `exec` + `kind = "list"`, request on stdin |
| row `action` | `actions[0]` with `run.exec` |
| row `secondary` | `actions[1]`, usually `run.plugin` with `after = "refresh"` |
| `query` row template | `[query_item]` |
| every command runs through `bash -lc` | only with `env = "login"` |

During the migration the launcher loads both: v0 modes from `modes.json`, and
v1 plugins from `plugins/`. When `pi` and `terminal` are ported, `modes.json`
and the v0 loader are removed.

## Open questions

1. **A Rust core process.** This draft keeps the launcher in QML as the host.
   Alternatively a long-running Rust core could own discovery, plugin
   processes, matching and caching, and talk to QML over one stdio pipe.
   That moves almost all logic into testable Rust, but adds a process to
   keep alive and another protocol between QML and the core. I'd start
   without it and add it only if QML-side process handling gets unwieldy.
2. **Global search.** Should a plugin be able to show items in the no-prefix
   search (`global = true`), for example pinned pi projects next to apps?
3. **Prefix style.** `pi:` and `>` coexist today. Should prefixes follow one
   convention (`name:`), with symbols only as aliases?
4. **Distribution.** Bundled plugins ship as binaries built by `cargo build`
   at install time. Is requiring a Rust toolchain to install acceptable, or
   should releases include prebuilt binaries?
