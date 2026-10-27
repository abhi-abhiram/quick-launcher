//! A plugin that lists its environment: one item per `QL_*` variable,
//! with the variable as id and its value as label, plus `cwd` for its
//! working directory.

use std::env;

use quick_launcher_core_fixtures::item;
use quick_launcher_plugin::{Item, Plugin};

struct Env;

impl Plugin for Env {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        let mut vars: Vec<(String, String)> = env::vars()
            .filter(|(name, _)| name.starts_with("QL_"))
            .collect();
        vars.sort();
        let mut items: Vec<Item> = vars
            .into_iter()
            .map(|(name, value)| item(name, value))
            .collect();
        items.push(item("cwd", env::current_dir()?.to_string_lossy()));
        Ok(items)
    }
}

fn main() {
    quick_launcher_plugin::run(Env)
}
