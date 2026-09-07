use std::path::Path;

fn fingerprint(path: &Path, bytes: &mut Vec<u8>) {
    if path.is_dir() {
        println!("cargo:rerun-if-changed={}", path.display());
        let mut entries = std::fs::read_dir(path)
            .expect("read emitter sources")
            .map(|entry| entry.expect("emitter source entry").path())
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries {
            fingerprint(&entry, bytes);
        }
    } else {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path.to_str().expect("UTF-8 emitter source path").as_bytes();
        let content = std::fs::read(path).expect("read emitter source");
        bytes.extend_from_slice(&(name.len() as u64).to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&(content.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&content);
    }
}

fn main() {
    let mut bytes = Vec::new();
    fingerprint(Path::new("src"), &mut bytes);
    fingerprint(Path::new("Cargo.toml"), &mut bytes);
    fingerprint(Path::new("build.rs"), &mut bytes);
    println!(
        "cargo:rustc-env=KILN_C_EMITTER_BUILD_ID={}",
        kiln_digest::Digest::hash(&bytes).to_hex()
    );
}
