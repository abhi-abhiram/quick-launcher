//! A plugin that takes too long. `list` never answers. `query` with the
//! text `linger` answers at once but then doesn't exit; any other text
//! sleeps that many milliseconds and then answers.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use quick_launcher_core_fixtures::{hang, item, write_pid};
use quick_launcher_plugin::{Item, Plugin, Query};

static LINGER: AtomicBool = AtomicBool::new(false);

struct Slow;

impl Plugin for Slow {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        hang()
    }

    fn query(&mut self, query: &Query) -> anyhow::Result<Vec<Item>> {
        if query.text() == "linger" {
            LINGER.store(true, Ordering::Relaxed);
        } else {
            thread::sleep(Duration::from_millis(query.text().parse()?));
        }
        Ok(vec![item("late", "Late")])
    }
}

fn main() {
    write_pid();
    quick_launcher_plugin::run(Slow);
    if LINGER.load(Ordering::Relaxed) {
        hang()
    }
}
