use vatra_models::variant::{RepoFile, group};

fn file(path: &str, size: u64) -> RepoFile {
    RepoFile { path: path.into(), size, sha256: Some(format!("sha-{path}")) }
}

#[test]
fn groups_split_files() {
    let variants = group(&[
        file("m-Q4_K_M-00002-of-00002.gguf", 5),
        file("m-Q4_K_M-00001-of-00002.gguf", 10),
        file("m-Q8_0.gguf", 20),
        file("README.md", 1),
    ]);
    assert_eq!(variants.len(), 2);
    assert_eq!(variants[0].name, "Q8_0", "best quality first");
    let q4 = &variants[1];
    assert_eq!(q4.size, 15);
    assert_eq!(q4.files[0].path, "m-Q4_K_M-00001-of-00002.gguf");
}

#[test]
fn mmproj_is_attachment() {
    let variants = group(&[file("m-Q4_K_M.gguf", 10), file("mmproj-F16.gguf", 2), file("mmproj-model-f16.gguf", 2)]);
    assert_eq!(variants.len(), 1);
    assert_eq!(variants[0].mmproj.len(), 2);
    assert_eq!(variants[0].size, 10);
}
