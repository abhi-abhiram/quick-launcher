# v1 implementation milestones

How [protocol.md](protocol.md) gets built, in small steps. Each milestone is
one branch and one PR into `main`, leaves `main` green, and ends with tests
that prove it. Section names in *italics* point into protocol.md.

The order gets something real running early: by M4 the `apps` plugin lists
your installed apps, and by M8 the core searches and launches them from a
terminal. Every milestone after that adds to a binary you can already run.

v0 (`Launcher.qml`, `modes.json`, `modes/*.py`) keeps working on `main`
until the last milestone. The Rust workspace grows next to it, and the
Omarchy shell doesn't use it until [M15](#m15--switch-the-omarchy-frontend),
which is the one-step switch the spec asks for (*Migrating from v0*).

## Overview

| #   | Milestone | What you can run afterwards | Depends on |
|-----|-----------|-----------------------------|------------|
| M1  | [Workspace and wire types](#m1--workspace-and-wire-types) | `cargo test` | — |
| M2  | [Matcher](#m2--matcher) | `cargo test` | M1 |
| M3  | [Plugin SDK](#m3--plugin-sdk) | the spec's `repos` example plugin, by piping a request into it | M1, M2 |
| M4  | [`apps` plugin](#m4--apps-plugin) | `apps` listing your real installed apps | M3 |
| M5  | [Platform](#m5--platform) | `cargo test` | M1 |
| M6  | [Manifests, config and discovery](#m6--manifests-config-and-discovery) | `cargo test` | M5 |
| M7  | [Plugin host: oneshot](#m7--plugin-host-oneshot) | `cargo test` | M3, M6 |
| M8  | [First runnable core: plain search](#m8--first-runnable-core-plain-search) | **search and launch your apps from a terminal** | M4, M7 |
| M9  | [Modes, prefixes and input](#m9--modes-prefixes-and-input) | entering a mode with `repo:` | M8 |
| M10 | [The full result list](#m10--the-full-result-list) | mode hints, Tab, query items | M9 |
| M11 | [Activation](#m11--activation) | plugin actions (pin, forget) | M10 |
| M12 | [`pi` plugin](#m12--pi-plugin) | `pi:` mode, from the terminal | M3 |
| M13 | [`terminal` plugin](#m13--terminal-plugin) | `>` mode, from the terminal | M3 |
| M14 | [Plugin host: persistent, streaming, cancel](#m14--plugin-host-persistent-streaming-cancel) | `cargo test` | M7, M11 |
| M15 | [Switch the Omarchy frontend](#m15--switch-the-omarchy-frontend) | **the shell launcher on v1** | M12–M14 |

"Depends on" lists what a milestone needs to be built and tested. Trying a
plugin through the core needs more: the core gains modes in M9, hints in
M10 and plugin actions in M11.

After M3, the `apps` plugin (M4) and the core's foundations (M5–M7) can be
built in parallel. `pi` and `terminal` (M12, M13) can be written any time
after M3, but you can only try them through the core once M9–M11 land. M14
comes late because no bundled plugin needs it; it is still part of v1.

## Trying the core: `tools/dev.py`

From M8 on, `python3 tools/dev.py` is how you run v1 before the shell uses
it. It builds the workspace, lays out each bundled plugin as
`target/plugins/<id>/` (`plugin.toml` plus binary), and starts the core
with `--bundled-plugins target/plugins`. Then it works as a minimal
frontend in your terminal:

```
> fire
 1. Firefox             Web Browser
 2. Firefox Developer…
> !1                    # run item 1's default action
```

Typing a line sends it as `input`, read in the mode the prompt shows
(`Repos> ` in a mode, `> ` in the plain search). `!N` activates item N's
default action, `!!N` its alternate, `<` sends `leave_mode` and `\t` sends
`complete`. It uses the real Omarchy platform, so `!1` really launches
Firefox. It's a second frontend speaking the public protocol, which is the check the spec's
*Testing* section asks for. The end-to-end test (`tests/e2e_core.py`) uses
the same client code with fixture plugins and the recording platform (see
M8).

`--bundled-plugins` is a development flag. It only replaces the bundled
plugin directory in `Dirs`; everything else still comes from `Platform`.
The plugin layout code lives in `tools/plugin_layout.py`, which `dev.py`
uses now and `install.py` imports in M15.

## Decisions made up front

The spec leaves these choices open. These are the defaults. Each one is
explained again at the start of the milestone that first uses it, and can
be changed then.

- **Rust edition 2024**, one Cargo workspace. `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings` and `cargo test` pass on every
  PR. `python3 tools/check.py`, added in M1, runs them along with the v0 test
  commands from the README, which keep passing until M15 deletes v0. Run it
  before pushing. A GitHub Actions workflow, on Tenki runners, runs the same
  commands on every PR and every push to `main`.
- **Crates**: `serde` + `serde_json` for JSON, `toml` for manifests and
  config. The SDK's API uses `anyhow::Result`, as the spec's examples do.
- **The matcher lives in the wire crate** (`quick_launcher_wire::matcher`),
  because the core and the SDK both need it and the SDK re-exports it. No
  fourth crate. protocol.md's *Repository layout* says so too.
- **The core uses `tokio`** for plugin processes, timeouts, and reading
  stdin while plugins answer. **The SDK does not**: it uses `std` and one
  reader thread, so a plugin binary stays small and plugin authors don't
  inherit an async runtime.
- **The session does no IO of its own.** It's a state machine that takes
  events (frontend messages, plugin responses, timeouts) and returns
  effects (send `state`, send `done`, send a plugin request, cancel one, run
  an action). The trickiest rules in the spec (stale input, `seq`,
  `[query_item]` refs) then become plain unit tests. M8 starts it small;
  M9–M11 grow it.
- **"Watched" plugin directories** are rescanned on `open` when a directory
  or manifest mtime changed. Users see what the spec describes ("takes
  effect the next time the launcher opens") without a file-watching
  dependency.
- **Matching keeps v0's scoring** (`textScore` in `LauncherModel.js`): exact,
  then prefix, then substring, then in-order subsequences, tighter ones
  first. v1 adds the `keywords` tier below `detail`.
- **Prefix parsing keeps v0's rule**: the longest matching prefix wins, then
  whitespace after it is dropped.
- **The dev client and the end-to-end test are Python**, using only the
  standard library, like the existing tests. Being a frontend in another
  language is the point (*Testing*).
- **Open questions** in protocol.md stay open. v1 ships without them:
  `apps` only launches (question 1), the bundled prefixes stay `pi:`, `>`
  and `term:` (question 2), and `install.py` builds from source (question 3).

## Milestones

### M1 — Workspace and wire types

Branch `feat/wire-types`.

**Goal.** The workspace exists, and the types the core and the SDK share are
defined once and tested against every JSON example in the spec.

**Scope** (*Wire format*, *Items*, *Actions*, *Icons*)

- Root `Cargo.toml` workspace with `crates/wire`, plus empty `crates/core`
  and `crates/plugin` so later PRs only add code. `.gitignore` covers
  `target/`.
- `tools/check.py`: runs `cargo fmt --check`, `cargo clippy --all-targets
  -- -D warnings`, `cargo test`, and the v0 tests (`python3 -m unittest`,
  `node --test`). `.github/workflows/ci.yml` runs the same commands on
  every PR and every push to `main`.
- `quick-launcher-wire`: `PROTOCOL` (= 1), `Request` (`list`, `query`,
  `activate`, `cancel`), `Response` (`items`, `result`, `error`, `more`),
  `Item`, `Action`, `Run` (the six forms), `After`, `Icon` (string or
  `glyph`/`name`/`path` object), `ActivateResult` with its defaults
  (`close: true`, `refresh: false`).
- Line framing helpers: read one line, enforce the 4 MiB limit, write one
  line.

**Not in scope.** Frontend protocol types (they're the core's, M8). Item
validation beyond what serde does (M7).

**Done when**

- Every wire JSON example in protocol.md round-trips through the types.
- Unknown fields are ignored; missing optional fields take their defaults.
- An `Icon` string parses as a glyph regardless of length.
- A `Run` with two forms, or none, is rejected.
- A line over 4 MiB is an error, not an allocation.
- `python3 tools/check.py` and the CI workflow pass on this PR.

### M2 — Matcher

Branch `feat/matcher`.

**Goal.** One matcher, used by the core and exported by the SDK.

**Scope** (*Filtering and ranking*)

- `score(text, query)`: port of v0's `textScore`, case-insensitive; an empty
  query matches everything.
- `filter(items, query)`: label, then detail, then keywords. A label match
  always outranks a detail or keyword match, and ties keep input order.
  Returns indices and scores so the core can merge results from several
  plugins.
- Helpers for mode hints: `starts_with` ignoring case, and "names the mode
  outright" (v0's `namesMode`).

**Done when**

- The `textScore`, `rowScore` and `filterRows` cases in v0's
  `tests/test_model.mjs` are ported and pass, plus keyword cases.
- Ties keep order, including an empty query returning everything in order.

### M3 — Plugin SDK

Branch `feat/plugin-sdk`.

**Goal.** A plugin can be written exactly as in the spec's examples.

**Scope** (*Plugin SDK*, *Environment*, *Transport*, *Requests*, *Responses*)

- `Plugin` trait with `list`, `query`, `activate`. Methods a plugin doesn't
  implement return an error response.
- Builders: `Item::new(id, label).detail().icon().badge().keywords()
  .disabled().score().action()`, `Action::new(id, title).run().plugin()
  .key().after()`, `Run::{exec, terminal, app, open, copy}` with `.cwd()`,
  `Outcome::{close, refresh, query}`. The sixth run form, `plugin`, comes
  from `Action::plugin()` rather than a `Run` constructor, as in the spec's
  example.
- `run(plugin)`: reads requests until EOF and answers each one, so the same
  binary works as `oneshot` and `persistent`. Writes only protocol lines to
  stdout.
- Panics in a method are caught and become an error response; the loop
  continues.
- `query` gets a handle with `partial(items)` (sends `more: true`) and
  `cancelled()` (set when a `cancel` for that request arrives, read by a
  background thread).
- `dirs()` reads `QL_CONFIG_DIR` / `QL_STATE_DIR` and creates them on first
  use. Re-exports the matcher.
- `examples/repos.rs`: the spec's *Examples* plugin, compiled by `cargo test`
  (and so by `tools/check.py`).

**Done when**

- Tests drive `run` over in-memory pipes: list, query, activate, a request
  for an unimplemented method, a panicking method followed by a good
  request, partial responses, a `cancel` observed by a running query, and a
  clean exit at EOF.
- `echo '{"v":1,"id":1,"method":"list","params":{}}' | ./repos` prints your
  repos as one response line.

### M4 — `apps` plugin

Branch `feat/plugin-apps`.

**Goal.** Installed apps are a global list plugin. The first real plugin,
runnable on its own.

**Scope** (*Apps are a plugin*, *Examples* table)

- Read desktop entries from the XDG data dirs, honouring precedence (a
  user entry hides a system one with the same id). Skip `NoDisplay` and
  `Hidden`. Label from `Name`, detail from `Comment` or `GenericName`,
  keywords from `Keywords`, icon as `{"name": …}` or `{"path": …}`.
- One action: `run.app` with the desktop entry id.
- `plugins/apps/plugin.toml`: `global = true`, `oneshot`, `session`.

**Done when**

- Tests over a temporary data dir cover precedence, hidden entries,
  localized-key fallback and icon forms.
- Piping a `list` request into the built binary prints your installed apps.

### M5 — Platform

Branch `feat/core-platform`.

**Goal.** Everything OS-specific in the core sits behind one trait.

**Scope** (*Platform*, *Rules that keep the core agnostic*)

- `Platform` trait and `Dirs` (bundled plugins, user plugins, config file,
  per-plugin config and state roots, home).
- `Omarchy` implementation, exactly as in the spec's table: `setsid
  uwsm-app`, `xdg-terminal-exec --dir`, `xdg-open`, `wl-copy`,
  `bash -lc 'exec "$@"'`, XDG directories with their usual fallbacks.
- `Recording` platform for tests: stores each call and never runs anything.
  It is compiled only under `#[cfg(any(test, feature = "recording-platform"))]`,
  so the core's unit tests get it without the feature and release builds
  never contain it. M8 uses the feature to put it in the binary for the e2e
  test.
- `run_action(platform, run, plugin)`: maps `exec`/`terminal`/`app`/`open`/
  `copy` to `Platform`. `cwd` defaults to home, relative `exec` argv paths
  resolve against the plugin folder, and `exec` and `terminal` go through
  `login_command` for `env = "login"` plugins. `plugin` runs are not handled
  here (M11).

**Done when**

- Each run form produces the expected recorded call, including the cwd
  default, relative paths and login wrapping (and its absence for `open`,
  `copy` and `app`).
- `Omarchy`'s argv builders are tested as pure functions; nothing is
  launched in tests.
- `crates/core` builds and its tests pass with no Quickshell, Hyprland or
  `uwsm-app` installed.

### M6 — Manifests, config and discovery

Branch `feat/core-discovery`.

**Goal.** The core knows which plugins exist and what they claim.

**Scope** (*Plugin discovery*, *Manifest*, *Kinds*, *Errors and timeouts*)

- Parse `plugin.toml`: required keys, defaults (`kind = "list"`,
  `lifetime = "oneshot"`, `env = "session"`, `timeout_ms = 2000`), the id
  pattern, at least one prefix, `global` and `[query_item]` only for
  `list`, unknown keys ignored.
- Skip and log: invalid manifest, folder name ≠ `id`, `protocol` ≠ core's
  (the log asks for a rebuild), missing `bin`.
- Discovery order: bundled, then user. A user plugin replaces a bundled one
  with the same id.
- `config.toml`: `disabled`, and `[plugins.<id>].prefixes` replacing the
  manifest's.
- Prefix conflicts: the plugin discovered first keeps the prefix, and a
  warning names both.
- Rescan when a directory or manifest changed since the last scan (used on
  `open`).

**Done when** tests over temporary directories cover each rule above,
including an override, a disabled plugin, a prefix override that creates a
conflict, and a rescan that picks up an added and a removed plugin.

### M7 — Plugin host: oneshot

Branch `feat/core-host-oneshot`.

**Goal.** The core can run a `oneshot` plugin and get its items back,
safely.

**Scope** (*Lifetimes*, *Environment*, *Transport*, *Errors and timeouts*)

- Fixture plugins built with the SDK in `crates/core/tests/fixtures/` (or a
  workspace member): one that lists, one that's slow, one that panics, one
  that prints garbage, one that streams, one that echoes its environment.
- Start the binary with the `QL_*` variables and the plugin folder as
  working directory, through `login_command` for `env = "login"`.
- Write the request, close stdin, use each response line as it arrives,
  and reap after the final response (killing after `timeout_ms`).
- Timeouts: `timeout_ms` applies to each gap between responses. On expiry,
  kill the process and report that it didn't respond.
- stderr forwarded to the core's log, prefixed with the plugin id.
- Response handling: an unparsable line is an error; items missing `id`,
  `label` or `actions` are dropped and logged and the rest kept; responses
  with a mismatched id are ignored.

**Done when** each fixture produces the expected items, error or timeout,
no test leaves a process behind, and the environment fixture shows every
`QL_*` variable with the right value.

### M8 — First runnable core: plain search

Branch `feat/core-binary`.

**Goal.** `quick-launcher-core` is a real program: it searches your apps
and launches them, driven from a terminal.

**Scope** (*Frontend protocol*, *Versions*, *Filtering and ranking*,
*Frontend items*)

- Frontend protocol types. `hello` → `ready` (always, even on a version
  mismatch, then log and exit non-zero). Unknown types and fields are
  ignored.
- The session, first version: `open` asks every global list plugin for its
  items; `input` with `mode: null` filters them with the matcher (discovery
  order, then plugin order on ties); `max_items` cap; `loading` and
  `error`; `seq` echoed.
- Frontend items: `ref` from plugin id and item id, icon resolved to an
  explicit object (defaulting to the plugin's icon), `run` and `after`
  stripped.
- `activate` for `Platform` run forms with `after = "close"`, then `done`.
  `close` ends the session.
- Main loop: frontend stdin, plugin host events and timers feed the
  session; effects are written to stdout or executed. A `state` is sent
  whenever a plugin answers. Rescan plugins on `open`. Log to stderr. EOF on
  stdin exits 0.
- `--bundled-plugins <dir>` development flag.
- `tools/dev.py`, the terminal client described
  [above](#trying-the-core-toolsdevpy), and `tools/plugin_layout.py`, which
  builds `target/plugins/<id>/` for it.
- `tests/e2e_core.py`: drives the built core with fixture plugins in
  temporary XDG directories and the recording platform, so nothing
  launches: hello, open, input, activate, close.
- The recording platform (gated since M5) reaches the binary only through
  the `recording-platform` cargo feature, which the e2e test builds with and
  release builds never enable. With it on, the core writes each `Platform` call to a file
  named by `QL_RECORD`. protocol.md's "no stubs" rule is about platforms
  that don't exist yet; this is a test double, and the feature keeps it
  out of what users run.

**Not in scope.** Modes and prefixes (M9), hints and query items (M10),
`plugin` actions and `refresh` (M11).

**Done when**

- The e2e test passes on a machine without Omarchy, Hyprland or
  Quickshell.
- A version-mismatched `hello` gets `ready`, then EOF with a non-zero exit.
- By hand: `python3 tools/dev.py`, type `fire`, `!1` launches Firefox.

### M9 — Modes, prefixes and input

Branch `feat/core-modes`.

**Goal.** Modes work, including fast typing.

**Scope** (*Frontend protocol*, *Reading input as the user saw it*, *The
query*, *Kinds*)

- Session state: active mode, text, last applied `seq`.
- `open` with or without `mode` resets to that mode with empty text and
  requests the mode's list plugin as well as the global ones.
- `input`: switch to `input.mode` first if it differs. Parse prefixes only
  when `mode` is `null`, and drop whitespace right after a prefix.
- `leave_mode`: put back the prefix used to enter the mode (or its first
  prefix), minus its last character.
- The query: `text` trimmed, used for filtering and sent as `text` in
  plugin requests; `state.text` keeps trailing whitespace.
- In a list mode, filter that plugin's items. For `query` plugins, send a
  `query` request whenever the query changes (and on entering the mode) and
  show the items as given, sorted by `score` when present.
- `tools/dev.py` shows the active mode in its prompt.

**Done when**

- Unit tests cover the spec's worked examples: `pi:` then `pi:l` both with
  `mode: null`; another mode's prefix typed inside a mode is search text;
  `leave_mode` after entering by prefix and by `open`; `seq` on the `state`
  answering `leave_mode`.
- The e2e test enters and leaves a fixture mode.
- By hand: `repo:` (the prefix in protocol.md's *Examples*) in
  `tools/dev.py` lists the `repos` example plugin's repos. `dev.py` lays
  the example out next to the bundled plugins.

### M10 — The full result list

Branch `feat/core-results`.

**Goal.** Every `state` carries the right items in the right order.

**Scope** (*Mode hints*, *Filtering and ranking*, *Items*, *Manifest*
`[query_item]`)

- Mode hints in the plain search, built as in the spec's table, and
  `complete` (the selected hint's mode, or the first hint's).
- Plain search order: hints the query names outright, then global items
  (match quality, then discovery order, then plugin order), then other
  hints.
- `[query_item]`: `{query}` filled into every string, shown first, hidden
  for an empty query or when a filtered item has exactly that label. Its
  `ref` can't collide with an item's and records the text it was built
  from.
- Activating a hint's `enter` action enters its mode and sends `state`,
  not `done`.

**Done when** unit tests reproduce v0's plain-search order (named modes,
then apps, then other modes), cover the `[query_item]` dedupe and ref
uniqueness, and `complete` with and without a selected hint.

### M11 — Activation

Branch `feat/core-activate`.

**Goal.** Choosing any action does what the spec says, and only that.

**Scope** (*Frontend → core*, *Actions*, *Responses*)

- An unknown ref (dropped by a newer state) is logged, runs nothing, and
  re-sends `state`.
- A disabled item's first action can't run; its other actions can.
- `after = "refresh"` re-requests the mode's items instead of sending
  `done`.
- `plugin` runs: send `activate` with `item` (or `null` and the recorded
  text for the `[query_item]`), then apply `result`: `close`, `refresh`,
  and `query` replacing the text. An `activate` error keeps the current
  items.
- `[query_item]` Platform runs use the recorded text, not the current one.

**Done when** unit tests, using the recording `Platform`, cover each branch
above, including a `[query_item]` activated after newer input arrived (it
runs with the recorded text), and the e2e test runs a `plugin` action with
`refresh`.

### M12 — `pi` plugin

Branch `feat/plugin-pi`.

**Goal.** Port `modes/pi.py` to Rust with no change users can see.

**Scope** (*Examples* table, *Environment*, *Migrating from v0*)

- Recent projects from pi's session history, honouring
  `PI_CODING_AGENT_DIR`, `PI_CODING_AGENT_SESSION_DIR` and `sessionDir`.
  Pinned first, then most recent.
- Rows look as they do in v0 (`rows()` in `modes/pi.py`): pinned items get
  the `󰐃` icon, and the notes `missing` and `last used` follow the path in
  `detail` (`~/Projects/x · last used`). No `badge` is set, so the spec's
  `"badge":"pinned"` example stays an example. Moving the notes into badges
  is a visible change and belongs in its own PR after v1. Missing pinned
  folders are also `disabled`.
- The default action opens pi in a terminal in the project. Pin and unpin
  are `plugin` actions with `refresh`.
- Pins live in `QL_CONFIG_DIR`. While that file doesn't exist, read the v0
  file, then the old pi-launcher file. The first write carries them forward,
  and v0 files are never written.
- Manifest: `env = "login"`, prefix `pi:`.

**Done when** `tests/test_pi_mode.py`'s cases are ported to Rust tests and
pass, plus the migration cases (read v0, first write carries forward, v0
file untouched), and `pi:` works in `tools/dev.py`, pinning included.

### M13 — `terminal` plugin

Branch `feat/plugin-terminal`.

**Goal.** Port `modes/terminal.py` to Rust.

**Scope** (*Examples* table, *Manifest* `[query_item]`, *Migrating from
v0*)

- History as items, most recent first. The default action runs the command
  again; the alternate (a `plugin` action with `refresh`) forgets it.
- `[query_item]` runs the typed command and records it in history through
  a `plugin` action. Commands run in an interactive bash in a new terminal,
  as in v0, with the shell in the plugin's own argv.
- History lives in `QL_STATE_DIR`, with the same v0 fallback and
  carry-forward rule as pi.
- Manifest: prefixes `>` and `term:`.
- A `plugin` action's `result` can carry a `run` the core starts before
  applying the rest (`Outcome::run` in the SDK). Without it, an action
  could record a command or open it in a terminal, not both. Running a
  history item uses it too, so the command moves to the top, as in v0.
- Commands name no `cwd`. v0 used the focused terminal's folder
  (`omarchy-cmd-terminal-cwd`), which a plugin can't ask for, so a
  follow-up before M15 made it the platform's choice: a `terminal` with
  no `cwd` opens in the focused terminal's folder on Omarchy, or home.

**Done when** `tests/test_terminal_mode.py`'s cases are ported and pass,
plus the migration cases, and `>` works in `tools/dev.py`.

### M14 — Plugin host: persistent, streaming, cancel

Branch `feat/core-host-persistent`.

**Goal.** `persistent` plugins and streamed answers work as specified.

**Scope** (*Lifetimes*, *Responses*, *Errors and timeouts*)

- Start on first use, keep running, unique request ids per process.
- A newer `query` supersedes the older one: only the newest request's
  responses are used, `cancel` is sent for the superseded request, and a
  superseded `oneshot` process is killed.
- Partial responses (`more: true`) are shown as they arrive, with
  `loading: true`; the final one ends the request. A timeout after a partial
  leaves its items in place.
- Exit handling: restart on the next request, at most three times a minute,
  then report the error until the core restarts. A timeout leaves the
  process running for the next request.
- Shutdown: close every persistent plugin's stdin and wait briefly.

**Done when** fixtures cover two queries in a row (the first dropped and
cancelled), a stream of partials then a final, a crash and restart, a
fourth crash in a minute staying failed, and shutdown leaving no process.

### M15 — Switch the Omarchy frontend

Branch `feat/omarchy-v1`. One PR, as the spec requires.

**Goal.** The shell launcher runs on the core, and v0 is gone.

**Scope** (*Frontend protocol*, *Repository layout*, *Migrating from v0*)

- Move `Launcher.qml` and `manifest.json` to `frontends/omarchy/`.
- `Launcher.qml` starts the core once (`keepLoaded`) and sends `hello`;
  `open` on show; `input` with `seq` on every change; `leave_mode` on
  Backspace in empty text; `complete` on Tab; `activate` on Enter,
  Shift+Enter, click, right-click and action `key`s; `close` on Escape or
  hide. It draws `state` following the stale-state rule, hides on `done`,
  and reports a major version mismatch as a notification.
- Delete the QML app search and the `AppLibrary` workaround, `modes.json`,
  `modes/*.py`, `LauncherModel.js` and their tests.
- `install.py`: checks for `cargo`, builds the workspace in release mode,
  installs the core binary and the bundled plugins to
  `$XDG_DATA_HOME/quick-launcher/plugins/<id>/` (importing
  `tools/plugin_layout.py`), then the shell plugin as today. `--uninstall` removes both.
- README: new install requirements, writing a plugin (pointing at the SDK
  and the `repos` example), updated data paths and development commands.
  protocol.md's status changes from draft to implemented.

**Done when** `tests/test_install.py` covers the new install layout,
`qmllint` and `omarchy plugin validate` pass, and a manual check on Omarchy
covers apps search and launch, `pi:` with pin and unpin, `>` with run and
forget, Tab and Backspace mode changes, opening straight into a mode with
`'{"mode":"pi"}'`, and fast typing of `pi:launch`.
