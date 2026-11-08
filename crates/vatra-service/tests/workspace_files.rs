use std::path::Path;
use vatra_core::{DocumentFormat, Store, WorkspaceError, parse_workspace_csv};
use vatra_service::workspace::files::{
    DocumentExportMode, export_document, export_document_with_mode, import_document,
};

#[test]
fn import_preserves_source_bytes_and_exports_each_format() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let p = store.create_project("A").unwrap();
    for (extension, format, body) in [
        ("txt", DocumentFormat::Text, "Ședință\nnotiță"),
        ("md", DocumentFormat::Markdown, "# Titlu\n\n**text**"),
        ("html", DocumentFormat::Html, "<p>text</p><script>throw 1</script>"),
        ("csv", DocumentFormat::Csv, "a,b\n\"x,y\",z\n"),
    ] {
        let source = dir.path().join(format!("source.{extension}"));
        std::fs::write(&source, body).unwrap();
        let item = import_document(&store, p, &source, format, 1).unwrap();
        assert_eq!(item.body, body);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), body);
        let output = dir.path().join(format!("export.{extension}"));
        export_document(&store, p, item.id, item.revision, &output).unwrap();
        if format == DocumentFormat::Csv {
            assert_eq!(
                parse_workspace_csv(&std::fs::read_to_string(output).unwrap()).unwrap(),
                parse_workspace_csv(body).unwrap()
            );
        } else {
            assert_eq!(std::fs::read_to_string(output).unwrap(), body);
        }
    }
}
#[test]
fn oversized_import_and_invalid_encoding_create_no_item() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let p = store.create_project("A").unwrap();
    let source = dir.path().join("bad.txt");
    for bytes in [vec![b'x'; 2 * 1024 * 1024 + 1], vec![0xff, 0xfe, 0xff]] {
        std::fs::write(&source, bytes).unwrap();
        assert!(import_document(&store, p, &source, DocumentFormat::Text, 1).is_err());
    }
    assert!(store.workspace_items(p, &[], false, None, 100).unwrap().is_empty());
}
#[test]
fn file_paths_and_extensions_are_checked_before_io() {
    let store = Store::open_in_memory().unwrap();
    let p = store.create_project("A").unwrap();
    for path in [
        r"\\server\share\x.md",
        r"\\?\C:\x.md",
        "//server/share/x.md",
        "relative.md",
        "C:/missing.exe",
    ] {
        assert!(
            matches!(
                import_document(&store, p, Path::new(path), DocumentFormat::Markdown, 1),
                Err(WorkspaceError::Invalid(_))
            ),
            "{path}"
        );
    }
}
#[test]
fn document_export_refuses_existing_files_and_stale_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let p = store.create_project("A").unwrap();
    let source = dir.path().join("note.md");
    std::fs::write(&source, "old").unwrap();
    let item = import_document(&store, p, &source, DocumentFormat::Markdown, 1).unwrap();
    assert!(export_document(&store, p, item.id, item.revision, &source).is_err());
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "old");
    store.update_item(item.id, None, Some("new"), None, 2).unwrap();
    let output = dir.path().join("export.md");
    assert!(matches!(
        export_document(&store, p, item.id, item.revision, &output),
        Err(WorkspaceError::Conflict(_))
    ));
    assert!(!output.exists());
}
#[test]
fn safe_csv_export_neutralizes_formulas_but_exact_export_preserves_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let p = store.create_project("A").unwrap();
    let source = dir.path().join("table.csv");
    let body = "=1+2,+3,-4,@SUM(A1)\n";
    std::fs::write(&source, body).unwrap();
    let item = import_document(&store, p, &source, DocumentFormat::Csv, 1).unwrap();
    let safe = dir.path().join("safe.csv");
    export_document(&store, p, item.id, 1, &safe).unwrap();
    assert_eq!(
        parse_workspace_csv(&std::fs::read_to_string(safe).unwrap()).unwrap()[0],
        vec!["'=1+2", "'+3", "'-4", "'@SUM(A1)"]
    );
    let exact = dir.path().join("exact.csv");
    export_document_with_mode(&store, p, item.id, 1, &exact, DocumentExportMode::Exact).unwrap();
    assert_eq!(std::fs::read_to_string(exact).unwrap(), body);
    assert_eq!(store.workspace_item(p, item.id, false).unwrap().body, body);
}
