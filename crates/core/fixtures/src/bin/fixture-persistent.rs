//! A `query` plugin meant to run `persistent`. The text `wait` answers only
//! once the request is cancelled, and logs that it was. `crash` exits with
//! status 3. `sleep N` sleeps N milliseconds and then answers. Any other
//! text answers at once with one item whose id is the text. Every item's
//! label is the process id, so a test can tell whether two answers came
//! from the same process. `list` answers with that one item too.

use std::time::{Duration, Instant};
use std::{process, thread};

use quick_launcher_core_fixtures::{FOREVER, item, write_pid};
use quick_launcher_plugin::{Item, Plugin, Query};

struct Persistent;

fn answer(id: &str) -> Vec<Item> {
    vec![item(id, process::id().to_string())]
}

impl Plugin for Persistent {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        Ok(answer("list"))
    }

    fn query(&mut self, query: &Query) -> anyhow::Result<Vec<Item>> {
        let text = query.text();
        if text == "wait" {
            let deadline = Instant::now() + FOREVER;
            while !query.cancelled() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            eprintln!("cancelled while waiting");
            return Ok(answer("waited"));
        }
        if text == "crash" {
            process::exit(3);
        }
        if let Some(ms) = text.strip_prefix("sleep ") {
            thread::sleep(Duration::from_millis(ms.parse()?));
            return Ok(answer("slept"));
        }
        Ok(answer(text))
    }
}

fn main() {
    write_pid();
    quick_launcher_plugin::run(Persistent)
}
