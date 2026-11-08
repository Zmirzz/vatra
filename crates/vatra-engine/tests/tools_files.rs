use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use vatra_core::ProcessGroup;
use vatra_engine::{
    Background, NoQuestions, Tool, ToolClass, ToolContext, ToolOutput, apply_patch_tool, edit_tool, glob_tool,
    grep_tool, read_tool, write_tool,
};

/// A folder whose name holds non-ASCII text, so every path test covers it.
fn workspace() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("Răzvan Test").tempdir().unwrap()
}

fn context(cwd: &Path, project_root: &Path) -> ToolContext {
    let (events, _) = mpsc::unbounded_channel();
    ToolContext {
        cwd: cwd.to_path_buf(),
        project_root: project_root.to_path_buf(),
        session_dir: cwd.to_path_buf(),
        read_files: Arc::new(Mutex::new(HashSet::new())),
        events,
        questions: Arc::new(NoQuestions),
        tree: Arc::new(ProcessGroup::new().unwrap()),
        background: Arc::new(Background::new()),
        hidden_env: vec![],
        memory_dir: None,
    }
}

fn project(dir: &Path) -> ToolContext {
    context(dir, dir)
}

fn mark_read(context: &ToolContext, path: &Path) {
    context.read_files.lock().unwrap().insert(path.to_path_buf());
}

/// A patch from its lines, with the markers around them. Lines are kept exact, so a leading space still counts.
fn patch_text(lines: &[&str]) -> String {
    let mut all = vec!["*** Begin Patch"];
    all.extend_from_slice(lines);
    all.push("*** End Patch");
    all.join("\n")
}

async fn call(tool: Arc<dyn Tool>, context: &ToolContext, arguments: Value) -> ToolOutput {
    tool.call(arguments, context).await
}

#[tokio::test]
async fn read_numbers_lines() {
    let dir = workspace();
    let file = dir.path().join("a.txt");
    fs::write(&file, "alpha\nbeta\n").unwrap();
    let context = project(dir.path());

    let out = call(read_tool(), &context, json!({"file_path": file})).await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(out.text, "     1\talpha\n     2\tbeta");
}

#[tokio::test]
async fn read_offset_limit() {
    let dir = workspace();
    let file = dir.path().join("lines.txt");
    fs::write(&file, "l1\nl2\nl3\nl4\nl5\n").unwrap();
    let context = project(dir.path());

    let out = call(read_tool(), &context, json!({"file_path": file, "offset": 2, "limit": 2})).await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(out.text, "     2\tl2\n     3\tl3\n\n(showing lines 2-3 of 5; use offset to read more)");
}

#[tokio::test]
async fn read_marks_file_as_read() {
    let dir = workspace();
    let file = dir.path().join("a.txt");
    fs::write(&file, "alpha\n").unwrap();
    let context = project(dir.path());

    call(read_tool(), &context, json!({"file_path": file})).await;

    assert!(context.read_files.lock().unwrap().contains(&file));
}

#[tokio::test]
async fn read_rejects_directory_with_clear_message() {
    let dir = workspace();
    let context = project(dir.path());

    let out = call(read_tool(), &context, json!({"file_path": dir.path()})).await;

    assert!(out.is_error);
    assert!(out.text.contains("is a directory"), "{}", out.text);
}

#[tokio::test]
async fn read_image_returns_image_part() {
    let dir = workspace();
    let file = dir.path().join("pixel.png");
    let bytes = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];
    fs::write(&file, bytes).unwrap();
    let context = project(dir.path());

    let out = call(read_tool(), &context, json!({"file_path": file})).await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(out.images.len(), 1);
    assert_eq!(out.images[0].media_type, "image/png");
    assert_eq!(out.images[0].base64, base64::engine::general_purpose::STANDARD.encode(bytes));
}

#[tokio::test]
async fn read_binary_is_refused() {
    let dir = workspace();
    let file = dir.path().join("data.bin");
    fs::write(&file, [0x00, 0x01, 0x02, 0x03]).unwrap();
    let context = project(dir.path());

    let out = call(read_tool(), &context, json!({"file_path": file})).await;

    assert!(out.is_error);
    assert!(out.text.contains("binary"), "{}", out.text);
}

#[tokio::test]
async fn read_class_inside_and_outside() {
    let inside = workspace();
    let outside = workspace();
    let context = project(inside.path());
    let tool = read_tool();

    let inside_file = inside.path().join("a.txt");
    let outside_file = outside.path().join("b.txt");
    let escaping = inside.path().join("..").join(outside.path().file_name().unwrap()).join("b.txt");

    assert_eq!(tool.class(&json!({"file_path": inside_file}), &context), ToolClass::Free);
    assert_eq!(tool.class(&json!({"file_path": outside_file}), &context), ToolClass::ReadOutside);
    assert_eq!(tool.class(&json!({"file_path": escaping}), &context), ToolClass::ReadOutside);
    assert!(tool.parallel_safe());
}

#[tokio::test]
async fn write_new_file() {
    let dir = workspace();
    let file = dir.path().join("new.txt");
    let context = project(dir.path());

    let out = call(write_tool(), &context, json!({"file_path": file, "content": "hello"})).await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
}

#[tokio::test]
async fn write_refuses_existing_unread() {
    let dir = workspace();
    let file = dir.path().join("old.txt");
    fs::write(&file, "old").unwrap();
    let context = project(dir.path());

    let out = call(write_tool(), &context, json!({"file_path": file, "content": "new"})).await;

    assert!(out.is_error);
    assert_eq!(fs::read_to_string(&file).unwrap(), "old");
}

#[tokio::test]
async fn write_overwrites_read_file() {
    let dir = workspace();
    let file = dir.path().join("old.txt");
    fs::write(&file, "old").unwrap();
    let context = project(dir.path());
    mark_read(&context, &file);

    let out = call(write_tool(), &context, json!({"file_path": file, "content": "new"})).await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(fs::read_to_string(&file).unwrap(), "new");
}

#[tokio::test]
async fn write_creates_parents() {
    let dir = workspace();
    let file = dir.path().join("nested").join("deeper").join("new.txt");
    let context = project(dir.path());

    let out = call(write_tool(), &context, json!({"file_path": file, "content": "deep"})).await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(fs::read_to_string(&file).unwrap(), "deep");
}

#[tokio::test]
async fn edit_unique_match() {
    let dir = workspace();
    let file = dir.path().join("a.txt");
    fs::write(&file, "one two three\n").unwrap();
    let context = project(dir.path());
    mark_read(&context, &file);

    let out = call(edit_tool(), &context, json!({"file_path": file, "old_string": "two", "new_string": "2"})).await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(fs::read_to_string(&file).unwrap(), "one 2 three\n");
}

#[tokio::test]
async fn edit_refuses_read_line_numbers_without_touching_the_file() {
    for replacement in ["     1\tconst value = 2;\n     2\tmodule.exports = value;", "     1const value = 2;\n     2module.exports = value;"] {
        let dir = workspace();
        let file = dir.path().join("code.js");
        let original = "const value = 1;\nmodule.exports = value;";
        fs::write(&file, original).unwrap();
        let context = project(dir.path());
        mark_read(&context, &file);
        let output = call(edit_tool(), &context, json!({"file_path":file,"old_string":original,"new_string":replacement})).await;
        assert!(output.is_error, "{}", output.text);
        assert_eq!(fs::read_to_string(&file).unwrap(), original);
    }
}

#[tokio::test]
async fn edit_refuses_inserting_a_whole_file_at_each_identifier() {
    let dir = workspace();
    let file = dir.path().join("code.js");
    let original = "function getUser() {}\nmodule.exports = { getUser };\n";
    fs::write(&file, original).unwrap();
    let context = project(dir.path());
    mark_read(&context, &file);
    let output = call(edit_tool(), &context, json!({"file_path":file,"old_string":"getUser","new_string":"function fetchUser() {}\nmodule.exports = { fetchUser };\n","replace_all":true})).await;
    assert!(output.is_error, "{}", output.text);
    assert_eq!(fs::read_to_string(file).unwrap(), original);
}

#[tokio::test]
async fn edit_allows_a_multiline_value_in_place_of_an_identifier() {
    let dir = workspace();
    let file = dir.path().join("text.txt");
    fs::write(&file,"HEADER\ntail\n").unwrap();
    let context = project(dir.path());
    mark_read(&context,&file);
    let output = call(edit_tool(),&context,json!({"file_path":file,"old_string":"HEADER","new_string":"first\nsecond"})).await;
    assert!(!output.is_error,"{}",output.text);
    assert_eq!(fs::read_to_string(file).unwrap(),"first\nsecond\ntail\n");
}

#[tokio::test]
async fn edit_allows_numbered_data_already_present_and_exact_whole_file_replacements() {
    let dir = workspace();
    let file = dir.path().join("data.txt");
    fs::write(&file, "     1\told\n     2\tdata").unwrap();
    let context = project(dir.path());
    mark_read(&context, &file);
    let output = call(edit_tool(), &context, json!({"file_path":file,"old_string":"     1\told\n     2\tdata","new_string":"     1\tnew\n     2\tdata"})).await;
    assert!(!output.is_error, "{}", output.text);
    assert_eq!(fs::read_to_string(file).unwrap(), "     1\tnew\n     2\tdata");
}

#[tokio::test]
async fn edit_zero_matches_says_so() {
    let dir = workspace();
    let file = dir.path().join("a.txt");
    fs::write(&file, "one two\n").unwrap();
    let context = project(dir.path());
    mark_read(&context, &file);

    let out = call(edit_tool(), &context, json!({"file_path": file, "old_string": "four", "new_string": "4"})).await;

    assert!(out.is_error);
    assert!(out.text.contains("not found"), "{}", out.text);
    assert_eq!(fs::read_to_string(&file).unwrap(), "one two\n");
}

#[tokio::test]
async fn edit_many_matches_says_how_many() {
    let dir = workspace();
    let file = dir.path().join("a.txt");
    fs::write(&file, "a a a\n").unwrap();
    let context = project(dir.path());
    mark_read(&context, &file);

    let out = call(edit_tool(), &context, json!({"file_path": file, "old_string": "a", "new_string": "b"})).await;

    assert!(out.is_error);
    assert!(out.text.contains("3 places"), "{}", out.text);
    assert_eq!(fs::read_to_string(&file).unwrap(), "a a a\n");
}

#[tokio::test]
async fn edit_replace_all() {
    let dir = workspace();
    let file = dir.path().join("a.txt");
    fs::write(&file, "a a a\n").unwrap();
    let context = project(dir.path());
    mark_read(&context, &file);

    let out = call(
        edit_tool(),
        &context,
        json!({"file_path": file, "old_string": "a", "new_string": "b", "replace_all": true}),
    )
    .await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(fs::read_to_string(&file).unwrap(), "b b b\n");
}

#[tokio::test]
async fn edit_preserves_crlf() {
    let dir = workspace();
    let file = dir.path().join("crlf.txt");
    fs::write(&file, "one\r\ntwo\r\nthree\r\n").unwrap();
    let context = project(dir.path());
    mark_read(&context, &file);

    let out =
        call(edit_tool(), &context, json!({"file_path": file, "old_string": "two\nthree", "new_string": "2\n3"})).await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(fs::read(&file).unwrap(), b"one\r\n2\r\n3\r\n");
}

#[tokio::test]
async fn edit_preserves_bom() {
    let dir = workspace();
    let file = dir.path().join("bom.txt");
    let mut original = vec![0xEF, 0xBB, 0xBF];
    original.extend_from_slice(b"hello world\n");
    fs::write(&file, &original).unwrap();
    let context = project(dir.path());
    mark_read(&context, &file);

    let out = call(edit_tool(), &context, json!({"file_path": file, "old_string": "hello", "new_string": "bye"})).await;

    assert!(!out.is_error, "{}", out.text);
    let mut expected = vec![0xEF, 0xBB, 0xBF];
    expected.extend_from_slice(b"bye world\n");
    assert_eq!(fs::read(&file).unwrap(), expected);
}

#[tokio::test]
async fn edit_requires_read_first() {
    let dir = workspace();
    let file = dir.path().join("a.txt");
    fs::write(&file, "one\n").unwrap();
    let context = project(dir.path());

    let out = call(edit_tool(), &context, json!({"file_path": file, "old_string": "one", "new_string": "two"})).await;

    assert!(out.is_error);
    assert_eq!(fs::read_to_string(&file).unwrap(), "one\n");
}

#[tokio::test]
async fn glob_sorted_by_mtime_capped_100() {
    let dir = workspace();
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for i in 0..120 {
        let file = dir.path().join(format!("f{i:03}.txt"));
        fs::write(&file, "x").unwrap();
        File::options().write(true).open(&file).unwrap().set_modified(base + Duration::from_secs(i)).unwrap();
    }
    let context = project(dir.path());

    let out = call(glob_tool(), &context, json!({"pattern": "*.txt", "path": dir.path()})).await;

    assert!(!out.is_error, "{}", out.text);
    let files: Vec<&str> = out.text.lines().filter(|line| line.ends_with(".txt")).collect();
    assert_eq!(files.len(), 100);
    assert!(files[0].ends_with("f119.txt"), "{}", files[0]);
    assert!(files[99].ends_with("f020.txt"), "{}", files[99]);
}

#[tokio::test]
async fn glob_respects_gitignore() {
    let dir = workspace();
    fs::write(dir.path().join(".gitignore"), "ignored.txt\nbuild/\n").unwrap();
    fs::write(dir.path().join("kept.txt"), "x").unwrap();
    fs::write(dir.path().join("ignored.txt"), "x").unwrap();
    fs::create_dir(dir.path().join("build")).unwrap();
    fs::write(dir.path().join("build").join("out.txt"), "x").unwrap();
    let context = project(dir.path());

    let out = call(glob_tool(), &context, json!({"pattern": "**/*.txt", "path": dir.path()})).await;

    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("kept.txt"), "{}", out.text);
    assert!(!out.text.contains("ignored.txt"), "{}", out.text);
    assert!(!out.text.contains("out.txt"), "{}", out.text);
}

/// Three files: a Rust file, and two text files. `alpha` appears on lines 2 of b.txt and 1 and 3 of c.txt.
fn grep_fixture(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let rust = dir.join("a.rs");
    let b = dir.join("b.txt");
    let c = dir.join("c.txt");
    fs::write(&rust, "fn main() {\n    println!(\"HELLO\");\n}\n").unwrap();
    fs::write(&b, "hello world\nalpha\nbeta\n").unwrap();
    fs::write(&c, "alpha\nbeta\nalpha beta\n").unwrap();
    (rust, b, c)
}

#[tokio::test]
async fn grep_modes() {
    let dir = workspace();
    let (_, b, c) = grep_fixture(dir.path());
    let context = project(dir.path());
    let (b, c) = (b.display(), c.display());

    let files = call(grep_tool(), &context, json!({"pattern": "alpha", "path": dir.path()})).await;
    assert_eq!(files.text, format!("{b}\n{c}"));

    let content =
        call(grep_tool(), &context, json!({"pattern": "alpha", "path": dir.path(), "output_mode": "content"})).await;
    assert_eq!(content.text, format!("{b}:2:alpha\n{c}:1:alpha\n{c}:3:alpha beta"));

    let count =
        call(grep_tool(), &context, json!({"pattern": "alpha", "path": dir.path(), "output_mode": "count"})).await;
    assert_eq!(count.text, format!("{b}:1\n{c}:2"));
}

#[tokio::test]
async fn grep_case_flag() {
    let dir = workspace();
    let (rust, b, _) = grep_fixture(dir.path());
    let context = project(dir.path());

    let sensitive = call(grep_tool(), &context, json!({"pattern": "hello", "path": dir.path()})).await;
    assert_eq!(sensitive.text, b.display().to_string());

    let insensitive = call(grep_tool(), &context, json!({"pattern": "hello", "path": dir.path(), "-i": true})).await;
    assert_eq!(insensitive.text, format!("{}\n{}", rust.display(), b.display()));
}

#[tokio::test]
async fn grep_multiline() {
    let dir = workspace();
    let (_, b, c) = grep_fixture(dir.path());
    let context = project(dir.path());

    let single = call(grep_tool(), &context, json!({"pattern": "alpha\\s+beta", "path": dir.path()})).await;
    assert_eq!(single.text, c.display().to_string());

    let multi =
        call(grep_tool(), &context, json!({"pattern": "alpha\\s+beta", "path": dir.path(), "multiline": true})).await;
    assert_eq!(multi.text, format!("{}\n{}", b.display(), c.display()));
}

#[tokio::test]
async fn grep_type_and_glob() {
    let dir = workspace();
    let (rust, b, c) = grep_fixture(dir.path());
    let context = project(dir.path());

    let by_type = call(grep_tool(), &context, json!({"pattern": ".", "path": dir.path(), "type": "rust"})).await;
    assert_eq!(by_type.text, rust.display().to_string());

    let by_glob = call(grep_tool(), &context, json!({"pattern": ".", "path": dir.path(), "glob": "*.txt"})).await;
    assert_eq!(by_glob.text, format!("{}\n{}", b.display(), c.display()));
}

#[tokio::test]
async fn grep_head_limit_offset() {
    let dir = workspace();
    let file = dir.path().join("n.txt");
    let lines: Vec<String> = (1..=10).map(|i| format!("line {i}")).collect();
    fs::write(&file, lines.join("\n")).unwrap();
    let context = project(dir.path());
    let prefix = format!("{}:", file.display());

    let page = call(
        grep_tool(),
        &context,
        json!({"pattern": "line", "path": dir.path(), "output_mode": "content", "offset": 3, "head_limit": 2}),
    )
    .await;
    let entries: Vec<&str> = page.text.lines().filter(|line| line.starts_with(&prefix)).collect();
    assert_eq!(entries, vec![format!("{prefix}4:line 4"), format!("{prefix}5:line 5")]);

    let unlimited = call(
        grep_tool(),
        &context,
        json!({"pattern": "line", "path": dir.path(), "output_mode": "content", "head_limit": 0}),
    )
    .await;
    assert_eq!(unlimited.text.lines().filter(|line| line.starts_with(&prefix)).count(), 10);
}

#[tokio::test]
async fn apply_patch_add_update_delete() {
    let dir = workspace();
    fs::write(dir.path().join("keep.txt"), "one\ntwo\nthree\n").unwrap();
    fs::write(dir.path().join("gone.txt"), "bye\n").unwrap();
    let context = project(dir.path());
    let patch = patch_text(&[
        "*** Add File: new.txt",
        "+fresh",
        "*** Update File: keep.txt",
        "@@",
        " one",
        "-two",
        "+TWO",
        " three",
        "*** Delete File: gone.txt",
    ]);

    let out = call(apply_patch_tool(), &context, json!({"input": patch})).await;

    assert!(!out.is_error, "{}", out.text);
    assert_eq!(fs::read_to_string(dir.path().join("new.txt")).unwrap(), "fresh\n");
    assert_eq!(fs::read_to_string(dir.path().join("keep.txt")).unwrap(), "one\nTWO\nthree\n");
    assert!(!dir.path().join("gone.txt").exists());
}

#[tokio::test]
async fn grep_context_lines() {
    let dir = workspace();
    let file = dir.path().join("lines.txt");
    fs::write(&file, "one\ntwo\nthree\nfour\nfive\nsix\n").unwrap();
    let context = project(dir.path());
    let shown = file.display();

    let around = call(
        grep_tool(),
        &context,
        json!({"pattern": "four", "path": file, "output_mode": "content", "-B": 1, "-A": 1}),
    )
    .await;
    assert_eq!(around.text, format!("{shown}-3-three\n{shown}:4:four\n{shown}-5-five"));

    let symmetric =
        call(grep_tool(), &context, json!({"pattern": "four", "path": file, "output_mode": "content", "context": 1}))
            .await;
    assert_eq!(symmetric.text, around.text);

    let after_only =
        call(grep_tool(), &context, json!({"pattern": "four", "path": file, "output_mode": "content", "-A": 1})).await;
    assert_eq!(after_only.text, format!("{shown}:4:four\n{shown}-5-five"));

    let without_numbers = call(
        grep_tool(),
        &context,
        json!({"pattern": "four", "path": file, "output_mode": "content", "-C": 1, "-n": false}),
    )
    .await;
    assert_eq!(without_numbers.text, format!("{shown}-three\n{shown}:four\n{shown}-five"));

    let groups =
        call(grep_tool(), &context, json!({"pattern": "one|five", "path": file, "output_mode": "content", "-C": 1}))
            .await;
    assert_eq!(groups.text, format!("{shown}:1:one\n{shown}-2-two\n--\n{shown}-4-four\n{shown}:5:five\n{shown}-6-six"));
}

#[tokio::test]
async fn grep_unknown_type_errors() {
    let dir = workspace();
    fs::write(dir.path().join("a.rs"), "fn main() {}\n").unwrap();
    let context = project(dir.path());

    let out = call(grep_tool(), &context, json!({"pattern": "main", "path": dir.path(), "type": "nosuchtype"})).await;

    assert!(out.is_error);
    assert!(out.text.contains("Unknown file type"), "{}", out.text);
}

#[tokio::test]
async fn write_class_inside_and_outside() {
    let inside = workspace();
    let outside = workspace();
    let context = project(inside.path());
    let tool = write_tool();

    assert_eq!(tool.class(&json!({"file_path": inside.path().join("new.txt")}), &context), ToolClass::EditInside);
    assert_eq!(tool.class(&json!({"file_path": outside.path().join("new.txt")}), &context), ToolClass::EditOutside);
    assert_eq!(tool.class(&json!({"content": "no path"}), &context), ToolClass::EditOutside);
    assert!(!tool.parallel_safe());
}

#[tokio::test]
async fn edit_class_inside_and_outside() {
    let inside = workspace();
    let outside = workspace();
    let context = project(inside.path());
    let tool = edit_tool();

    assert_eq!(tool.class(&json!({"file_path": inside.path().join("a.txt")}), &context), ToolClass::EditInside);
    assert_eq!(tool.class(&json!({"file_path": outside.path().join("a.txt")}), &context), ToolClass::EditOutside);
    assert!(!tool.parallel_safe());
}

#[tokio::test]
async fn glob_class_inside_and_outside() {
    let inside = workspace();
    let outside = workspace();
    let context = project(inside.path());
    let tool = glob_tool();

    assert_eq!(tool.class(&json!({"pattern": "*"}), &context), ToolClass::Free);
    assert_eq!(tool.class(&json!({"pattern": "*", "path": inside.path()}), &context), ToolClass::Free);
    assert_eq!(tool.class(&json!({"pattern": "*", "path": outside.path()}), &context), ToolClass::ReadOutside);
    assert!(tool.parallel_safe());
}

#[tokio::test]
async fn grep_class_inside_and_outside() {
    let inside = workspace();
    let outside = workspace();
    let context = project(inside.path());
    let tool = grep_tool();

    assert_eq!(tool.class(&json!({"pattern": "x"}), &context), ToolClass::Free);
    assert_eq!(tool.class(&json!({"pattern": "x", "path": inside.path()}), &context), ToolClass::Free);
    assert_eq!(tool.class(&json!({"pattern": "x", "path": outside.path()}), &context), ToolClass::ReadOutside);
    assert!(tool.parallel_safe());
}

#[tokio::test]
async fn apply_patch_class_inside_and_outside() {
    let inside = workspace();
    let outside = workspace();
    let context = project(inside.path());
    let tool = apply_patch_tool();
    let add_outside = format!("*** Add File: {}", outside.path().join("new.txt").display());
    let move_out = format!("*** Move to: {}", outside.path().join("moved.txt").display());

    let inside_add = patch_text(&["*** Add File: new.txt", "+x"]);
    let outside_add = patch_text(&[add_outside.as_str(), "+x"]);
    let move_away = patch_text(&["*** Update File: keep.txt", move_out.as_str()]);

    assert_eq!(tool.class(&json!({"input": inside_add}), &context), ToolClass::EditInside);
    assert_eq!(tool.class(&json!({"input": outside_add}), &context), ToolClass::EditOutside);
    assert_eq!(tool.class(&json!({"input": move_away}), &context), ToolClass::EditOutside);
    assert_eq!(tool.class(&json!({"input": "not a patch"}), &context), ToolClass::EditOutside);
    assert_eq!(tool.class(&json!({}), &context), ToolClass::EditOutside);
    assert!(!tool.parallel_safe());
}

#[cfg(unix)]
fn make_dir_link(link: &Path, target: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(unix)]
fn make_file_link(link: &Path, target: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

/// A link to a folder: a symlink when the privilege allows it, otherwise a junction, which needs none.
#[cfg(windows)]
fn make_dir_link(link: &Path, target: &Path) -> bool {
    if std::os::windows::fs::symlink_dir(target, link).is_ok() {
        return true;
    }
    std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
fn make_file_link(link: &Path, target: &Path) -> bool {
    std::os::windows::fs::symlink_file(target, link).is_ok()
}

#[tokio::test]
async fn symlink_inside_project_pointing_outside_is_outside() {
    let project_dir = workspace();
    let outside = workspace();
    fs::write(outside.path().join("secret.txt"), "hidden\n").unwrap();
    let link = project_dir.path().join("link");
    if !make_dir_link(&link, outside.path()) {
        eprintln!("skipping: neither a symlink nor a junction could be created here");
        return;
    }
    let context = project(project_dir.path());
    let through = link.join("secret.txt");

    assert_eq!(read_tool().class(&json!({"file_path": through}), &context), ToolClass::ReadOutside);
    assert_eq!(write_tool().class(&json!({"file_path": through}), &context), ToolClass::EditOutside);
    assert_eq!(edit_tool().class(&json!({"file_path": through}), &context), ToolClass::EditOutside);
    assert_eq!(glob_tool().class(&json!({"pattern": "*", "path": link}), &context), ToolClass::ReadOutside);
    assert_eq!(grep_tool().class(&json!({"pattern": "x", "path": link}), &context), ToolClass::ReadOutside);
    let delete_through = patch_text(&["*** Delete File: link/secret.txt"]);
    assert_eq!(apply_patch_tool().class(&json!({"input": delete_through}), &context), ToolClass::EditOutside);
}

#[tokio::test]
async fn dangling_symlink_inside_project_pointing_outside_is_outside() {
    let project_dir = workspace();
    let outside = workspace();
    let dangling = project_dir.path().join("dangling.txt");
    if !make_file_link(&dangling, &outside.path().join("missing.txt")) {
        eprintln!("skipping: a file symlink could not be created here");
        return;
    }
    let context = project(project_dir.path());

    assert_eq!(write_tool().class(&json!({"file_path": dangling}), &context), ToolClass::EditOutside);
}

#[tokio::test]
async fn apply_patch_context_mismatch_changes_nothing() {
    let dir = workspace();
    fs::write(dir.path().join("keep.txt"), "one\ntwo\nthree\n").unwrap();
    let context = project(dir.path());
    let patch =
        patch_text(&["*** Add File: fresh.txt", "+x", "*** Update File: keep.txt", "@@", " nope", "-two", "+TWO"]);

    let out = call(apply_patch_tool(), &context, json!({"input": patch})).await;

    assert!(out.is_error);
    assert!(out.text.contains("Could not find these lines"), "{}", out.text);
    assert!(!dir.path().join("fresh.txt").exists());
    assert_eq!(fs::read_to_string(dir.path().join("keep.txt")).unwrap(), "one\ntwo\nthree\n");
}

#[tokio::test]
async fn edit_not_found_tells_how_to_correct_the_call() {
    let dir = workspace();
    let file = dir.path().join("a.txt");
    fs::write(&file, "one two\n").unwrap();
    let context = project(dir.path());
    mark_read(&context, &file);

    let out = call(edit_tool(), &context, json!({"file_path": file, "old_string": "     1\tone two", "new_string": "x"})).await;

    assert!(out.is_error);
    assert!(out.text.contains("line numbers"), "{}", out.text);
    assert!(out.text.contains("replace_all"), "{}", out.text);
}

#[tokio::test]
async fn edit_description_asks_for_short_text_and_names_replace_all() {
    let description = edit_tool().def().description;
    assert!(description.contains("one line"), "{description}");
    assert!(description.contains("replace_all"), "{description}");
    assert!(description.contains("line numbers"), "{description}");
}

#[tokio::test]
async fn edit_new_string_text_says_it_replaces_only_old_string_with_an_example() {
    let def = edit_tool().def();
    let text = format!("{} {}", def.description, def.parameters);
    assert!(text.contains("replaces only old_string"), "{text}");
    assert!(text.contains("getUser") && text.contains("fetchUser"), "{text}");
    assert!(text.contains("no line numbers"), "{text}");
}
