//! The `projects` plugin binary. The core starts it with `QL_STATE_DIR` set;
//! to try it by hand, set that too:
//! `echo '{"v":1,"id":1,"method":"query","params":{"text":"~/"}}' | QL_STATE_DIR=/tmp/projects target/release/projects-plugin`
//! (saved projects go in that folder).

use quick_launcher_projects::{Env, Projects};

fn main() {
    match Env::from_process() {
        Ok(env) => quick_launcher_plugin::run(Projects(env)),
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}
