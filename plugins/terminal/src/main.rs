//! The `terminal` plugin binary. The core starts it with `QL_STATE_DIR` set;
//! to try it by hand, set that too:
//! `echo '{"v":1,"id":1,"method":"list","params":{}}' | QL_STATE_DIR=/tmp/terminal target/release/terminal-plugin`
//! (history goes in that folder).

use quick_launcher_terminal::{Env, Terminal};

fn main() {
    match Env::from_process() {
        Ok(env) => quick_launcher_plugin::run(Terminal(env)),
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}
