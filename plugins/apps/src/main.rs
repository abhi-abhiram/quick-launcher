//! The `apps` plugin binary. Try it with
//! `echo '{"v":1,"id":1,"method":"list","params":{}}' | target/release/apps`.

#[cfg(not(target_os = "macos"))]
fn main() {
    use quick_launcher_apps::{Apps, Env};
    quick_launcher_plugin::run(Apps(Env::from_process()))
}

#[cfg(target_os = "macos")]
fn main() {
    use quick_launcher_apps::macos::{MacApps, roots_from_process};
    quick_launcher_plugin::run(MacApps(roots_from_process()))
}
