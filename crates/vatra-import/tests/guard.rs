mod support;

use std::fs;
use std::path::{Path, PathBuf};

use support::{MARKER, write_text};
use vatra_import::{GuardError, GuardedFs, is_forbidden};

fn home() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    (tmp, home)
}

#[test]
fn forbidden_names_rejected() {
    let (_tmp, home) = home();
    let claude = home.join(".claude");

    assert!(is_forbidden(&claude.join(".credentials.json")));
    assert!(is_forbidden(&home.join(".codex").join("auth.json")));
    assert!(is_forbidden(&claude.join("mcp-needs-auth-cache.json")));
    assert!(is_forbidden(&claude.join("mcp-oauth-locks").join("x")));
    assert!(is_forbidden(&claude.join("plugins").join("cache").join("auth.json")));
    assert!(!is_forbidden(&claude.join("settings.json")));
    assert!(!is_forbidden(&claude.join("skills").join("greet").join("SKILL.md")));

    write_text(
        &claude.join(".credentials.json"),
        &format!("{{\"token\":\"{MARKER}x\"}}"),
    );
    let fs_guard = GuardedFs::new();
    let error = fs_guard.read_to_string(&claude.join(".credentials.json")).unwrap_err();
    assert!(matches!(error, GuardError::Forbidden { .. }), "got {error:?}");
}

#[test]
fn case_insensitive_on_windows() {
    let (_tmp, home) = home();
    let claude = home.join(".claude");

    assert!(is_forbidden(&claude.join("AUTH.JSON")));
    assert!(is_forbidden(&claude.join(".Credentials.Json")));
    assert!(is_forbidden(&claude.join("Mcp-OAuth-Locks").join("x")));
    assert!(is_forbidden(&claude.join("Mcp-Needs-Auth-Cache.json")));
}

#[test]
fn token_and_credential_names_directly_in_config_folders_are_forbidden() {
    let (_tmp, home) = home();

    assert!(is_forbidden(&home.join(".claude").join("my-token.json")));
    assert!(is_forbidden(&home.join(".codex").join("Credential-store")));
    assert!(!is_forbidden(
        &home.join("projects").join("token-tool").join("SKILL.md")
    ));
    // Only direct children are refused: a plugin's own files with these words in their names still load.
    assert!(!is_forbidden(
        &home.join(".claude").join("plugins").join("my-token.txt")
    ));
    assert!(!is_forbidden(
        &home
            .join(".claude")
            .join("plugins")
            .join("cache")
            .join("superpowers")
            .join("tests")
            .join("token-usage.py")
    ));
}

#[test]
fn config_roots_refuse_direct_token_names_whatever_they_are_called() {
    let (_tmp, home) = home();
    let custom = home.join("custom-cfg");
    write_text(&custom.join("my-token.json"), "{}");
    write_text(&custom.join("plugins").join("token-usage.py"), "print(1)");

    let plain = GuardedFs::new();
    assert!(plain.read_to_string(&custom.join("my-token.json")).is_ok());

    let rooted = plain.with_config_roots(std::slice::from_ref(&custom));
    assert!(matches!(
        rooted.read_to_string(&custom.join("my-token.json")),
        Err(GuardError::Forbidden { .. })
    ));
    assert!(
        rooted
            .read_to_string(&custom.join("plugins").join("token-usage.py"))
            .is_ok()
    );
}

#[test]
fn dotdot_and_symlink_cannot_reach_forbidden() {
    let (_tmp, home) = home();
    let claude = home.join(".claude");
    write_text(
        &claude.join(".credentials.json"),
        &format!("{{\"k\":\"{MARKER}dotdot\"}}"),
    );
    write_text(&claude.join("mcp-oauth-locks").join("lock.json"), "{}");

    assert!(is_forbidden(
        &claude.join("plugins").join("..").join(".credentials.json")
    ));
    assert!(is_forbidden(
        &home.join("x").join("..").join(".claude").join(".credentials.json")
    ));

    let fs_guard = GuardedFs::new();
    let dotdot = claude.join("plugins").join("..").join(".credentials.json");
    assert!(matches!(
        fs_guard.read_to_string(&dotdot),
        Err(GuardError::Forbidden { .. })
    ));

    // A file link needs symlink rights, which Windows grants only to administrators or developer-mode machines; where
    // it cannot be made, only the folder link below is checked.
    let link = home.join("notes.json");
    if link_file(&claude.join(".credentials.json"), &link) {
        assert!(matches!(
            fs_guard.read_to_string(&link),
            Err(GuardError::Forbidden { .. })
        ));
    }

    let dir_link = home.join("locks");
    assert!(
        link_dir(&claude.join("mcp-oauth-locks"), &dir_link),
        "cannot make a folder link"
    );
    assert!(matches!(
        fs_guard.read_to_string(&dir_link.join("lock.json")),
        Err(GuardError::Forbidden { .. })
    ));
}

#[cfg(unix)]
fn link_file(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn link_file(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_file(target, link).is_ok()
}

#[cfg(unix)]
fn link_dir(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

/// A directory junction, which needs no administrator rights, unlike a directory symbolic link.
#[cfg(windows)]
fn link_dir(target: &Path, link: &Path) -> bool {
    std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn walk_skips_forbidden_silently() {
    let (_tmp, home) = home();
    let plugin = home.join(".claude").join("plugins").join("cache").join("p");
    write_text(
        &plugin.join("skills").join("greet").join("SKILL.md"),
        "---\ndescription: hi\n---\nhi\n",
    );
    write_text(&plugin.join("auth.json"), &format!("{{\"k\":\"{MARKER}walk\"}}"));
    write_text(&plugin.join("mcp-oauth-locks").join("x"), "lock");

    let walked = GuardedFs::new().walk(&plugin).expect("the walk itself does not fail");

    assert!(walked.files.iter().any(|path| path.ends_with("SKILL.md")));
    assert!(walked.files.iter().all(|path| !is_forbidden(path)));
    assert!(walked.skipped.iter().any(|path| path.ends_with("auth.json")));
    assert!(walked.skipped.iter().any(|path| path.ends_with("mcp-oauth-locks")));
}

#[test]
fn only_guard_touches_std_fs() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for entry in fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        checked += 1;
        if ["guard.rs", "apply.rs", "context.rs"].contains(&name.as_str()) {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("std::fs::"),
            "{name} reaches the file system without GuardedFs"
        );
    }
    assert!(checked > 0);
}
