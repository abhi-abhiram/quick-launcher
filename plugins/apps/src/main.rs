//! The `apps` plugin binary. Try it with
//! `echo '{"v":1,"id":1,"method":"list","params":{}}' | target/release/apps`.

use quick_launcher_apps::{Apps, Env};

fn main() {
    quick_launcher_plugin::run(Apps(Env::from_process()))
}
