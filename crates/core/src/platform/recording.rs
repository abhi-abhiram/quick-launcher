//! A platform for tests: it stores each call and never runs anything.
//!
//! Compiled only for the core's own tests and under the
//! `recording-platform` feature, so release builds never contain it.
//! With the feature, the core binary uses it when `QL_RECORD` names a file,
//! and writes each call there as one JSON line for the end-to-end test.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;

use super::{Dirs, Platform, non_empty};

/// One call a [`Recording`] received. As a JSON line it reads
/// `{"spawn":{"argv":[…],"cwd":"…"}}` or `{"copy":"text"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Call {
    Spawn {
        argv: Vec<String>,
        cwd: Option<PathBuf>,
    },
    Terminal {
        argv: Vec<String>,
        cwd: Option<PathBuf>,
    },
    LaunchApp(String),
    Open(String),
    Copy(String),
}

/// Records calls in order. `login_command` puts `login` in front of argv,
/// so a wrapped command is easy to tell apart.
#[derive(Debug)]
pub struct Recording {
    dirs: Dirs,
    calls: Mutex<Vec<Call>>,
    file: Option<Mutex<File>>,
}

impl Recording {
    pub fn new(dirs: Dirs) -> Recording {
        Recording {
            dirs,
            calls: Mutex::new(Vec::new()),
            file: None,
        }
    }

    /// A recording that also appends each call to `path` as a JSON line,
    /// creating the file if needed.
    pub fn to_file(dirs: Dirs, path: &Path) -> io::Result<Recording> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Recording {
            file: Some(Mutex::new(file)),
            ..Recording::new(dirs)
        })
    }

    /// Every call so far, oldest first.
    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: Call) -> io::Result<()> {
        if let Some(file) = &self.file {
            let mut line = serde_json::to_vec(&call).expect("calls always encode");
            line.push(b'\n');
            file.lock().unwrap().write_all(&line)?;
        }
        self.calls.lock().unwrap().push(call);
        Ok(())
    }
}

impl Platform for Recording {
    fn spawn(&self, argv: &[String], cwd: Option<&Path>) -> io::Result<()> {
        self.record(Call::Spawn {
            argv: non_empty(argv)?.to_vec(),
            cwd: cwd.map(Path::to_path_buf),
        })
    }

    fn terminal(&self, argv: &[String], cwd: Option<&Path>) -> io::Result<()> {
        self.record(Call::Terminal {
            argv: non_empty(argv)?.to_vec(),
            cwd: cwd.map(Path::to_path_buf),
        })
    }

    fn launch_app(&self, id: &str) -> io::Result<()> {
        self.record(Call::LaunchApp(id.to_owned()))
    }

    fn open(&self, target: &str) -> io::Result<()> {
        self.record(Call::Open(target.to_owned()))
    }

    fn copy(&self, text: &str) -> io::Result<()> {
        self.record(Call::Copy(text.to_owned()))
    }

    fn login_command(&self, argv: &[String]) -> Vec<String> {
        std::iter::once("login".to_owned())
            .chain(argv.iter().cloned())
            .collect()
    }

    fn dirs(&self) -> &Dirs {
        &self.dirs
    }
}
