//! `dirs()` reads the folders the core passes and creates them. This is its
//! own test binary because it sets environment variables.

use std::env;

use quick_launcher_plugin::dirs;

#[test]
fn dirs_come_from_the_environment_and_are_created() {
    let root = env::temp_dir().join(format!("ql-plugin-dirs-{}", std::process::id()));
    let (config, state) = (root.join("config/notes"), root.join("state/notes"));

    // SAFETY: this binary's only test, so no other thread reads the
    // environment meanwhile.
    unsafe {
        env::remove_var("QL_CONFIG_DIR");
        env::remove_var("QL_STATE_DIR");
    }
    let error = dirs().unwrap_err().to_string();
    assert!(error.contains("QL_CONFIG_DIR"), "{error}");

    unsafe {
        env::set_var("QL_CONFIG_DIR", &config);
        env::set_var("QL_STATE_DIR", &state);
    }
    let dirs = dirs().unwrap();
    assert_eq!(
        (dirs.config.as_path(), dirs.state.as_path()),
        (config.as_path(), state.as_path())
    );
    assert!(config.is_dir() && state.is_dir());

    std::fs::remove_dir_all(&root).unwrap();
}
