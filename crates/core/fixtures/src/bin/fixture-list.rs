//! A well-behaved list plugin: two items, and a `plugin` action that asks
//! for a refresh. It logs one line to stderr per request.

use quick_launcher_core_fixtures::item;
use quick_launcher_plugin::{Activate, Item, Outcome, Plugin};

struct List;

impl Plugin for List {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        eprintln!("listing two items");
        Ok(vec![item("one", "One"), item("two", "Two")])
    }

    fn activate(&mut self, activate: Activate) -> anyhow::Result<Outcome> {
        eprintln!("activating {:?} on {:?}", activate.action, activate.item);
        Ok(Outcome::refresh())
    }
}

fn main() {
    quick_launcher_plugin::run(List)
}
