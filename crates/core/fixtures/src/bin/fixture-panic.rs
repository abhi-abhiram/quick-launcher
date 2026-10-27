//! A plugin whose `list` panics.

use quick_launcher_plugin::{Item, Plugin};

struct Panics;

impl Plugin for Panics {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        panic!("the list is on fire")
    }
}

fn main() {
    quick_launcher_plugin::run(Panics)
}
