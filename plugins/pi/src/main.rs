//! The `pi` plugin binary. The core starts it with `QL_CONFIG_DIR` set; to
//! try it by hand, set that too:
//! `echo '{"v":1,"id":1,"method":"list","params":{}}' | QL_CONFIG_DIR=/tmp/pi target/release/pi-plugin`
//! (pins go in that folder).

use quick_launcher_pi::{Env, Pi};

fn main() {
    match Env::from_process() {
        Ok(env) => quick_launcher_plugin::run(Pi(env)),
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}
