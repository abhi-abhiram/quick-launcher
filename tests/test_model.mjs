import assert from "node:assert/strict"
import { createRequire } from "node:module"
import { readFileSync } from "node:fs"
import test from "node:test"

const require = createRequire(import.meta.url)
const Model = require("../LauncherModel.js")
const modes = Model.validModes(JSON.parse(readFileSync(new URL("../modes.json", import.meta.url))))
const pi = modes.find(mode => mode.id === "pi")
const terminal = modes.find(mode => mode.id === "terminal")

test("modes.json declares pi and terminal modes", () => {
  assert.deepEqual(modes.map(mode => mode.id), ["pi", "terminal"])
})

test("a complete prefix switches mode and keeps the rest as the query", () => {
  assert.deepEqual(Model.parsePrefix("pi:", modes), { mode: pi, query: "" })
  assert.deepEqual(Model.parsePrefix("PI: my proj", modes), { mode: pi, query: "my proj" })
  assert.deepEqual(Model.parsePrefix(">git status", modes), { mode: terminal, query: "git status" })
  assert.deepEqual(Model.parsePrefix("term: ls", modes), { mode: terminal, query: "ls" })
})

test("text without a complete prefix stays in apps", () => {
  assert.equal(Model.parsePrefix("pi", modes), null)
  assert.equal(Model.parsePrefix("firefox", modes), null)
  assert.equal(Model.parsePrefix(" pi:", modes), null)
})

test("the longest matching prefix wins", () => {
  const short = { id: "a", prefixes: ["t"], list: ["x"] }
  const long = { id: "b", prefixes: ["term:"], list: ["x"] }
  assert.equal(Model.parsePrefix("term: ls", [short, long]).mode, long)
})

test("partial prefixes and names suggest modes", () => {
  assert.deepEqual(Model.modeHints("p", modes), [pi])
  assert.deepEqual(Model.modeHints("ter", modes), [terminal])
  assert.deepEqual(Model.modeHints("", modes), [])
  assert.deepEqual(Model.modeHints("firefox", modes), [])
})

test("only naming a mode outright puts its hint first", () => {
  assert.equal(Model.namesMode("pi", pi), true)
  assert.equal(Model.namesMode("Terminal", terminal), true)
  assert.equal(Model.namesMode("term", terminal), true)
  assert.equal(Model.namesMode("p", pi), false)
  assert.equal(Model.namesMode("te", terminal), false)
})

test("text scores prefer exact, then prefix, then substring, then subsequence", () => {
  assert.equal(Model.textScore("pi-launcher", ""), 0)
  assert.equal(Model.textScore("pi", "pi"), 0)
  assert.equal(Model.textScore("pi-launcher", "pi"), 1)
  assert.equal(Model.textScore("quick-launcher", "launch"), 2)
  assert.ok(Model.textScore("quick-launcher", "qlnch") > 2)
  assert.equal(Model.textScore("quick-launcher", "xyz"), -1)
})

test("filtering ranks label matches above detail matches and keeps ties in order", () => {
  const rows = [
    { label: "webnav", detail: "~/Projects/webnav" },
    { label: "tools", detail: "~/Projects/tools" },
    { label: "projects-site", detail: "~/Work/projects-site" },
  ]
  assert.deepEqual(Model.filterRows(rows, "proj").map(row => row.label), ["projects-site", "webnav", "tools"])
  assert.deepEqual(Model.filterRows(rows, "").map(row => row.label), ["webnav", "tools", "projects-site"])
  assert.deepEqual(Model.filterRows(rows, "zzz"), [])
})

test("the query is substituted into whole arguments without a shell", () => {
  assert.deepEqual(Model.fillArgs(["run", "{query}"], "echo $(whoami); rm x"), ["run", "echo $(whoami); rm x"])
})

test("the typed command row comes first unless a row already matches it", () => {
  const typed = Model.queryRow(terminal, " git status ", [])
  assert.equal(typed.label, "git status")
  assert.deepEqual(typed.action, ["python3", "modes/terminal.py", "run", "git status"])
  assert.equal(Model.queryRow(terminal, "git status", [{ label: "git status" }]), null)
  assert.equal(Model.queryRow(terminal, "  ", []), null)
  assert.equal(Model.queryRow(pi, "anything", []), null)
})

test("malformed modes and rows are dropped", () => {
  assert.deepEqual(Model.validModes({ modes: [{ id: "x", prefixes: [], list: ["a"] }, null] }), [])
  assert.deepEqual(Model.validModes(null), [])
  const rows = Model.parseRows(JSON.stringify({ rows: [{ label: "ok", action: ["a"] }, { label: "no action" }, { action: ["a"] }] }))
  assert.deepEqual(rows.map(row => row.label), ["ok"])
  assert.throws(() => Model.parseRows("not json"))
})
