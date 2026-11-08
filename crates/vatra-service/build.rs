use std::{fs, path::Path};
fn walk(root: &Path, dir: &Path, output: &mut String) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(Result::unwrap).collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, output);
        } else {
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            output.push_str(&format!(
                "({name:?}, include_bytes!({:?})),\n",
                path.canonicalize().unwrap()
            ));
        }
    }
}
fn main() {
    let root = Path::new("../../parity");
    println!("cargo:rerun-if-changed=../../parity");
    let mut output = "pub const FILES: &[(&str, &[u8])] = &[\n".to_string();
    walk(root, root, &mut output);
    output.push_str("];\n");
    fs::write(
        Path::new(&std::env::var_os("OUT_DIR").unwrap()).join("parity_assets.rs"),
        output,
    )
    .unwrap();
}
