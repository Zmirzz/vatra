use std::ffi::OsStr;
use vatra_core::node_package_runner;
#[test]
fn windows_npm_runners_resolve_to_node_scripts_without_a_shell() {
    let dir = tempfile::tempdir().unwrap();
    let node = dir.path().join("node.exe");
    std::fs::write(&node, "").unwrap();
    let script = dir.path().join("node_modules/npm/bin/npx-cli.js");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, "").unwrap();
    let args = vec![
        "-y".into(),
        "@example/server@1.2.3".into(),
        "argument with spaces & symbols".into(),
    ];
    let found = node_package_runner("npx", &args, dir.path().as_os_str())
        .unwrap()
        .unwrap();
    assert_eq!(found.0, node);
    assert_eq!(std::path::PathBuf::from(&found.1[0]), script);
    assert_eq!(&found.1[1..], &args);
    assert!(
        node_package_runner("server.exe", &[], OsStr::new(""))
            .unwrap()
            .is_none()
    );
    assert!(node_package_runner("npm", &[], dir.path().as_os_str()).is_err());
}
