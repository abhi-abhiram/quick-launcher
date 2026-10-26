//! A platform for tests: it stores each call and never runs anything.
//!
//! Compiled only for the core's own tests and under the
//! `recording-platform` feature, so release builds never contain it.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::{Dirs, Platform, non_empty};

/// One call a [`Recording`] received.
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

impl Recording {
    pub fn new(dirs: Dirs) -> Recording {
        Recording {
            dirs,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Every call so far, oldest first.
    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: Call) -> io::Result<()> {
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
