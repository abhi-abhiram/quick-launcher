// Pure launcher logic shared by Launcher.qml and the node tests.

// The mode whose prefix starts `text`, preferring the longest prefix, and the
// query typed after it. Returns null while no prefix is complete.
function parsePrefix(text, modes) {
  var lower = String(text || "").toLowerCase()
  var best = null
  for (var i = 0; i < modes.length; i++) {
    var prefixes = modes[i].prefixes || []
    for (var j = 0; j < prefixes.length; j++) {
      var prefix = String(prefixes[j]).toLowerCase()
      if (prefix && lower.indexOf(prefix) === 0 && (!best || prefix.length > best.length))
        best = { mode: modes[i], length: prefix.length }
    }
  }
  if (!best) return null
  return { mode: best.mode, query: String(text).slice(best.length).replace(/^\s+/, "") }
}

// Modes the user may be starting to type, by prefix or by name.
function modeHints(text, modes) {
  var query = String(text || "").trim().toLowerCase()
  if (!query) return []
  return modes.filter(function(mode) {
    if (String(mode.name || "").toLowerCase().indexOf(query) === 0) return true
    return (mode.prefixes || []).some(function(prefix) {
      return String(prefix).toLowerCase().indexOf(query) === 0
    })
  })
}

// Whether `text` names the mode outright ("pi", "term", "terminal"), so its
// hint should come before the apps rather than after them.
function namesMode(text, mode) {
  var query = String(text || "").trim().toLowerCase()
  if (!query) return false
  if (String(mode.name || "").toLowerCase() === query) return true
  return (mode.prefixes || []).some(function(prefix) {
    return String(prefix).toLowerCase().replace(/:$/, "") === query
  })
}

// Lower is better; -1 when `query` does not match `text`. Every term must
// appear as a substring or, failing that, as an in-order subsequence, and
// tighter subsequences rank higher.
function textScore(text, query) {
  var name = String(text || "").toLowerCase()
  var needle = String(query || "").toLowerCase().trim()
  if (!needle) return 0
  if (name === needle) return 0
  if (name.indexOf(needle) === 0) return 1
  if (name.indexOf(needle) >= 0) return 2
  var terms = needle.split(/\s+/).filter(function(term) { return term.length > 0 })
  var total = 3
  for (var t = 0; t < terms.length; t++) {
    var term = terms[t]
    if (name.indexOf(term) >= 0) continue
    var cursor = 0
    var first = -1
    var last = -1
    for (var i = 0; i < term.length; i++) {
      var found = name.indexOf(term[i], cursor)
      if (found < 0) return -1
      if (first < 0) first = found
      last = found
      cursor = found + 1
    }
    total += 10 + last - first - term.length + 1
  }
  return total
}

// A match on the label always outranks one found only in the detail.
var DETAIL_PENALTY = 1000

function rowScore(row, query) {
  var score = textScore(row.label, query)
  if (score >= 0) return score
  score = textScore(row.detail, query)
  return score >= 0 ? score + DETAIL_PENALTY : -1
}

// Matching rows, best first; ties keep the order the mode listed them in.
function filterRows(rows, query) {
  if (!String(query || "").trim()) return rows.slice()
  var scored = []
  for (var i = 0; i < rows.length; i++) {
    var score = rowScore(rows[i], query)
    if (score >= 0) scored.push({ row: rows[i], score: score, index: i })
  }
  scored.sort(function(a, b) { return a.score - b.score || a.index - b.index })
  return scored.map(function(item) { return item.row })
}

// Substitutes the query into each argument; the argv never passes through a
// shell, so the query cannot inject anything.
function fillArgs(args, query) {
  return (args || []).map(function(arg) { return String(arg).split("{query}").join(query) })
}

// The row that acts on the typed text itself (e.g. "run this command"), shown
// first unless a listed row already has exactly that label.
function queryRow(mode, query, rows) {
  var text = String(query || "").trim()
  if (!mode.query || !text) return null
  for (var i = 0; i < rows.length; i++) if (rows[i].label === text) return null
  return {
    label: fillArgs([mode.query.label || "{query}"], text)[0],
    detail: fillArgs([mode.query.detail || ""], text)[0],
    icon: mode.query.icon || mode.icon || "",
    action: fillArgs(mode.query.action, text)
  }
}

// Modes from modes.json that are complete enough to use.
function validModes(data) {
  var modes = data && Array.isArray(data.modes) ? data.modes : []
  return modes.filter(function(mode) {
    return mode && typeof mode.id === "string" && Array.isArray(mode.prefixes)
      && mode.prefixes.length > 0 && Array.isArray(mode.list) && mode.list.length > 0
  })
}

// Rows from a mode's list command, keeping only well-formed ones.
function parseRows(text) {
  var rows = JSON.parse(text).rows
  if (!Array.isArray(rows)) return []
  return rows.filter(function(row) {
    return row && typeof row.label === "string" && row.label && Array.isArray(row.action)
  })
}

if (typeof module !== "undefined") {
  module.exports = {
    parsePrefix: parsePrefix,
    modeHints: modeHints,
    namesMode: namesMode,
    textScore: textScore,
    rowScore: rowScore,
    filterRows: filterRows,
    fillArgs: fillArgs,
    queryRow: queryRow,
    validModes: validModes,
    parseRows: parseRows
  }
}
