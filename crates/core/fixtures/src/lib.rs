//! Fixture plugins for the core's plugin host tests
//! (`crates/core/tests/host.rs`), one binary each in `src/bin/`. Each
//! does one thing a real plugin might, good or bad. None is ever
//! installed.

use std::time::Duration;
use std::{env, fs, process, thread};

use quick_launcher_plugin::{Action, Item, Run};

/// Long enough that only being killed ends it.
pub const FOREVER: Duration = Duration::from_secs(600);

/// An item with one `copy` action.
pub fn item(id: impl Into<String>, label: impl Into<String>) -> Item {
    let id = id.into();
    Item::new(&id, label).action(Action::new("copy", "Copy").run(Run::copy(&id)))
}

/// Writes this process's id to `$QL_STATE_DIR/pid`, so a test can check
/// that the process is gone after it was killed.
pub fn write_pid() {
    let dir = env::var_os("QL_STATE_DIR").expect("QL_STATE_DIR is set");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        std::path::Path::new(&dir).join("pid"),
        process::id().to_string(),
    )
    .unwrap();
}

/// Writes the pid and then never returns.
pub fn hang() -> ! {
    write_pid();
    loop {
        thread::sleep(FOREVER);
    }
}
