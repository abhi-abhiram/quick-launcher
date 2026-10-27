//! A `query` plugin that streams. The text `N` sends N partial answers
//! 200 ms apart, the i-th with i items, then a final one with N + 1. The
//! text `stall` sends one partial answer and then hangs.

use std::thread;
use std::time::Duration;

use quick_launcher_core_fixtures::{hang, item, write_pid};
use quick_launcher_plugin::{Item, Plugin, Query};

struct Stream;

fn items(n: usize) -> Vec<Item> {
    (1..=n)
        .map(|i| item(i.to_string(), format!("Item {i}")))
        .collect()
}

impl Plugin for Stream {
    fn query(&mut self, query: &Query) -> anyhow::Result<Vec<Item>> {
        if query.text() == "stall" {
            query.partial(items(1))?;
            hang()
        }
        let n: usize = query.text().parse()?;
        for i in 1..=n {
            query.partial(items(i))?;
            thread::sleep(Duration::from_millis(200));
        }
        Ok(items(n + 1))
    }
}

fn main() {
    write_pid();
    quick_launcher_plugin::run(Stream)
}
