//! The plugin from *Examples* in `docs/protocol.md`, as written there: git
//! repos under `~/Projects`, opened in VS Code.
//!
//! ```sh
//! cargo build --release --example repos
//! echo '{"v":1,"id":1,"method":"list","params":{}}' | target/release/examples/repos
//! ```

use quick_launcher_plugin::{Action, Item, Plugin, Run};
use std::{env, fs, path::PathBuf};

struct Repos;

impl Plugin for Repos {
    fn list(&mut self) -> anyhow::Result<Vec<Item>> {
        let root = PathBuf::from(env::var("HOME")?).join("Projects");
        Ok(fs::read_dir(root)?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.join(".git").is_dir())
            .map(|path| {
                let dir = path.display().to_string();
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                Item::new(&dir, name)
                    .detail(&dir)
                    .action(Action::new("open", "Open in VS Code").run(Run::exec(["code", &dir])))
            })
            .collect())
    }
}

fn main() {
    quick_launcher_plugin::run(Repos)
}
