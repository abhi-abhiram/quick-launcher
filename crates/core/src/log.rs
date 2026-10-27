//! The core's log. It goes to stderr; tests collect it instead.

use std::fmt;
use std::sync::{Arc, Mutex};

/// Where the core writes log lines. Cheap to clone and shareable between
/// tasks.
#[derive(Clone)]
pub struct Log(Arc<dyn Fn(&str) + Send + Sync>);

impl Log {
    /// Writes each line to stderr.
    pub fn stderr() -> Log {
        Log(Arc::new(|line| eprintln!("quick-launcher-core: {line}")))
    }

    /// Keeps each line in memory, for tests. The returned handle reads them.
    pub fn memory() -> (Log, Lines) {
        let lines = Lines::default();
        let sink = lines.clone();
        let log = Log(Arc::new(move |line| {
            sink.0.lock().unwrap().push(line.to_owned());
        }));
        (log, lines)
    }

    pub fn write(&self, line: impl fmt::Display) {
        (self.0)(&line.to_string());
    }
}

impl fmt::Debug for Log {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Log")
    }
}

/// The lines a [`Log::memory`] log received.
#[derive(Debug, Clone, Default)]
pub struct Lines(Arc<Mutex<Vec<String>>>);

impl Lines {
    /// Every line so far, oldest first.
    pub fn get(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}
