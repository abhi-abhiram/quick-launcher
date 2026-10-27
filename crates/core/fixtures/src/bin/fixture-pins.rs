//! A list plugin with `plugin` actions, kept in `QL_STATE_DIR`. It lists
//! `one`, `two` and every text its `[query_item]`'s `add` action recorded.
//! `pin` and `unpin` mark an item with a `pinned` badge. Every action
//! asks for a refresh, except `pin` on an item already pinned, which fails.

use std::fs;
use std::path::PathBuf;

use anyhow::bail;
use quick_launcher_core_fixtures::item;
use quick_launcher_plugin::{Action, Activate, Item, Outcome, Plugin};

struct Pins {
    state: PathBuf,
}

impl Pins {
    fn read(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.state.join(name))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    fn write(&self, name: &str, lines: &[String]) -> anyhow::Result<()> {
        Ok(fs::write(self.state.join(name), lines.join("\n"))?)
    }
}

impl Plugin for Pins {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        let pinned = self.read("pinned");
        let ids = ["one".to_owned(), "two".to_owned()]
            .into_iter()
            .chain(self.read("added"));
        Ok(ids
            .map(|id| {
                let (badge, action) = if pinned.contains(&id) {
                    (Some("pinned"), "unpin")
                } else {
                    (None, "pin")
                };
                let mut item = item(&id, &id).action(Action::new(action, action).plugin());
                if let Some(badge) = badge {
                    item = item.badge(badge);
                }
                item
            })
            .collect())
    }

    fn activate(&mut self, activate: Activate) -> anyhow::Result<Outcome> {
        eprintln!(
            "activating {:?} on {:?} with {:?}",
            activate.action, activate.item, activate.text
        );
        let mut pinned = self.read("pinned");
        match (activate.item, activate.action.as_str()) {
            (None, "add") => {
                let mut added = self.read("added");
                added.push(activate.text);
                self.write("added", &added)?;
            }
            (Some(id), "pin") if pinned.contains(&id) => bail!("{id} is already pinned"),
            (Some(id), "pin") => {
                pinned.push(id);
                self.write("pinned", &pinned)?;
            }
            (Some(id), "unpin") => {
                pinned.retain(|pin| *pin != id);
                self.write("pinned", &pinned)?;
            }
            (item, action) => bail!("no action {action:?} on {item:?}"),
        }
        Ok(Outcome::refresh())
    }
}

fn main() {
    let state = quick_launcher_plugin::dirs()
        .expect("started by the core")
        .state;
    quick_launcher_plugin::run(Pins { state })
}
