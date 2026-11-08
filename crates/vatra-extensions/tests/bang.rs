use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use vatra_extensions::{BangForm, Catalog, Origin, Scope, bang_spans};

/// The form and command of each span, in order.
fn found(body: &str) -> Vec<(BangForm, String)> {
    bang_spans(body).into_iter().map(|span| (span.form, span.command)).collect()
}

fn inline(command: &str) -> (BangForm, String) {
    (BangForm::Inline, command.into())
}

fn fenced(command: &str) -> (BangForm, String) {
    (BangForm::Fenced, command.into())
}

#[test]
fn prose_exclamation_before_backtick_is_not_a_command() {
    let line = "- **Special characters:** Escape shell metacharacters (`&`, `$`, `(`, `)`, `!`): `adb shell input text \"AT\\&T\"`";
    assert_eq!(found(line), vec![]);
}

#[test]
fn example_in_fenced_block_is_not_a_command() {
    let body = "```\n!`npm test $1`\n```\n~~~\n!`rm -rf x`\n~~~\n```bash\necho !`ls`\n```\n";
    assert_eq!(found(body), vec![]);
}

#[test]
fn example_in_inline_code_is_not_a_command() {
    let body = "Use the syntax `!`npm test $1`` in your skill.\nAlso `!`ls`` and wow!`echo glued`\n";
    assert_eq!(found(body), vec![]);
}

#[test]
fn line_start_and_after_space_are_commands() {
    let body = "!`echo start`\nSay !`echo after space` now\n\tindented: x !`echo tab` ok\nwow!`echo glued`";
    let spans = bang_spans(body);
    assert_eq!(
        spans.iter().map(|span| (span.form, span.command.clone())).collect::<Vec<_>>(),
        vec![inline("echo start"), inline("echo after space"), inline("echo tab")]
    );
    let texts: Vec<&str> = spans.iter().map(|span| &body[span.range.clone()]).collect();
    assert_eq!(texts, vec!["!`echo start`", "!`echo after space`", "!`echo tab`"]);
}

#[test]
fn fenced_bang_block_is_a_command() {
    let body = "Before\n```!\necho one\necho two\n```\nAfter\n";
    let spans = bang_spans(body);
    assert_eq!(found(body), vec![fenced("echo one\necho two")]);
    assert_eq!(&body[spans[0].range.clone()], "```!\necho one\necho two\n```\n");
}

#[test]
fn catalog_and_runner_agree() {
    let body = "Intro: `!`npm test $1`` and (`&`, `!`): ok\n\
                !`echo line`\n\
                Say !`echo space` now\n\
                ```\n!`echo example`\n```\n\
                ```!\necho fenced\n```\n\
                `!`echo code``\n";
    let expected = vec![inline("echo line"), inline("echo space"), fenced("echo fenced")];
    assert_eq!(found(body), expected);

    let fixture = fixture();
    let source = tempfile::tempdir().unwrap();
    let skill = skill_folder(source.path(), "mixed", body);
    fixture
        .catalog
        .add_folder(&skill, Origin::Manual, Scope::Global, false)
        .unwrap();
    let entries = fixture.catalog.list().unwrap();
    assert_eq!(entries.len(), 1, "{entries:#?}");
    let listed: Vec<(String, String)> = entries[0]
        .runs_code
        .iter()
        .map(|code| (code.what.clone(), code.command.clone()))
        .collect();
    let parsed: Vec<(String, String)> = expected
        .iter()
        .map(|(form, command)| {
            let what = match form {
                BangForm::Inline => "inline command",
                BangForm::Fenced => "fenced command",
            };
            (what.to_string(), command.clone())
        })
        .collect();
    assert_eq!(listed, parsed);
}

struct Fixture {
    _dir: TempDir,
    catalog: Catalog,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let catalog = Catalog::open(&data.join("vatra.db"), &data).unwrap();
    Fixture { _dir: dir, catalog }
}

/// A skill folder named `name` whose body is `body`.
fn skill_folder(root: &Path, name: &str, body: &str) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\ndescription: a test skill\n---\n{body}\n"),
    )
    .unwrap();
    dir
}
