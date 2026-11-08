use std::path::Path;

use vatra_extensions::{SubstituteInput, substitute};

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| name.to_string()).collect()
}

fn run(body: &str, arguments: &str, argument_names: &[String]) -> String {
    let input = SubstituteInput {
        arguments,
        argument_names,
        skill_dir: Path::new("/skills/demo"),
        session_id: "sess-1",
        plugin_root: None,
        project_dir: None,
        plugin_data: None,
        effort: None,
    };
    substitute(body, &input)
}

#[test]
fn arguments_whole() {
    assert_eq!(
        run("Review: $ARGUMENTS", "src/main.rs --fast", &[]),
        "Review: src/main.rs --fast"
    );
}

#[test]
fn arguments_indexed_zero_based() {
    assert_eq!(
        run("$ARGUMENTS[0] then $ARGUMENTS[1]", "alpha beta", &[]),
        "alpha then beta"
    );
}

#[test]
fn dollar_n() {
    assert_eq!(
        run("$0 and $1, then $2", "alpha beta", &[]),
        "alpha and beta, then "
    );
}

#[test]
fn named_arguments() {
    let names = names(&["file", "mode"]);
    assert_eq!(
        run("Open $file in $mode, not $other", "a.rs fast", &names),
        "Open a.rs in fast, not $other"
    );
}

#[test]
fn skill_dir() {
    assert_eq!(
        run("Run ${CLAUDE_SKILL_DIR}/notes.md", "", &[]),
        "Run /skills/demo/notes.md"
    );
}

#[test]
fn session_id() {
    assert_eq!(
        run("Session ${CLAUDE_SESSION_ID}", "", &[]),
        "Session sess-1"
    );
}

#[test]
fn plugin_root() {
    let input = SubstituteInput {
        arguments: "",
        argument_names: &[],
        skill_dir: Path::new("/skills/demo"),
        session_id: "sess-1",
        plugin_root: Some(Path::new("/plugins/ops")),
        project_dir: None,
        plugin_data: None,
        effort: None,
    };
    assert_eq!(
        substitute("Load ${CLAUDE_PLUGIN_ROOT}/tools", &input),
        "Load /plugins/ops/tools"
    );

    assert_eq!(
        run("Load ${CLAUDE_PLUGIN_ROOT}/tools", "", &[]),
        "Load ${CLAUDE_PLUGIN_ROOT}/tools"
    );
}

#[test]
fn escaped_dollar_stays() {
    assert_eq!(
        run(r"Cost \$ARGUMENTS and $ARGUMENTS", "x", &[]),
        "Cost $ARGUMENTS and x"
    );
    assert_eq!(
        run(r"Only \$ARGUMENTS here", "x", &[]),
        "Only $ARGUMENTS here\n\nARGUMENTS: x"
    );
}

#[test]
fn appends_when_no_placeholder() {
    assert_eq!(
        run("Do the thing.", "fix the bug", &[]),
        "Do the thing.\n\nARGUMENTS: fix the bug"
    );
}

#[test]
fn no_append_without_arguments() {
    assert_eq!(run("Do the thing.", "", &[]), "Do the thing.");
}

#[test]
fn quoted_args_shell_rules() {
    let body = "$0|$1|$2";
    assert_eq!(
        run(body, r#""hello world" 'it works' plain"#, &[]),
        "hello world|it works|plain"
    );
    assert_eq!(run(body, r#""say \"hi\"" x y"#, &[]), r#"say "hi"|x|y"#);
    assert_eq!(run(body, r#""" b c"#, &[]), "|b|c");
}

#[test]
fn path_with_spaces_inserted_verbatim() {
    let input = SubstituteInput {
        arguments: "",
        argument_names: &[],
        skill_dir: Path::new("/my skills/demo"),
        session_id: "sess-1",
        plugin_root: None,
        project_dir: None,
        plugin_data: None,
        effort: None,
    };
    assert_eq!(
        substitute("${CLAUDE_SKILL_DIR}/run.sh", &input),
        "/my skills/demo/run.sh"
    );
}

#[test]
fn project_dir_plugin_data_and_effort() {
    let input = SubstituteInput {
        arguments: "",
        argument_names: &[],
        skill_dir: Path::new("/skills/demo"),
        session_id: "sess-1",
        plugin_root: None,
        project_dir: Some(Path::new("/work/app")),
        plugin_data: Some(Path::new("/data/ops")),
        effort: Some("high"),
    };
    assert_eq!(
        substitute(
            "${CLAUDE_PROJECT_DIR}|${CLAUDE_PLUGIN_DATA}|${CLAUDE_EFFORT}",
            &input
        ),
        "/work/app|/data/ops|high"
    );
}

#[test]
fn unset_claude_values_stay_literal() {
    assert_eq!(
        run(
            "${CLAUDE_PROJECT_DIR} ${CLAUDE_PLUGIN_DATA} ${CLAUDE_EFFORT}",
            "",
            &[]
        ),
        "${CLAUDE_PROJECT_DIR} ${CLAUDE_PLUGIN_DATA} ${CLAUDE_EFFORT}"
    );
}

#[test]
fn values_are_not_substituted_twice() {
    assert_eq!(run("$ARGUMENTS", "$ARGUMENTS", &[]), "$ARGUMENTS");
}
