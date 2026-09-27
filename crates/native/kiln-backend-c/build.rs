use std::path::Path;

fn fingerprint(path: &Path, bytes: &mut Vec<u8>, excluded: Option<&Path>) {
    if excluded.is_some_and(|excluded| path == excluded) {
        return;
    }
    // These modules are declared #[cfg(test)] by the backend and IR crates. Editing a fixture
    // cannot change production emission, and must not invalidate every native artifact.
    if path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "src")
        && path
            .file_name()
            .is_some_and(|name| name == "tests" || name == "tests.rs")
    {
        return;
    }
    if path.is_dir() {
        println!("cargo:rerun-if-changed={}", path.display());
        let mut entries = std::fs::read_dir(path)
            .expect("read emitter sources")
            .map(|entry| entry.expect("emitter source entry").path())
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries {
            fingerprint(&entry, bytes, excluded);
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

/// Whether `byte` may appear in a property key the prelude spells as a C literal.
fn key_byte(byte: u8, first: bool) -> bool {
    byte.is_ascii_alphabetic()
        || byte == b'_'
        || byte == b'$'
        || byte == b'@'
        || (!first && (byte.is_ascii_digit() || byte == b':' || byte == b'.' || byte == b'-'))
}

/// The property keys the prelude C spells as literals (ADR 0071; realms issue 08).
///
/// A key a realm makes at run time that no table registered is the realm's own: interned in its
/// heap, and every shape built with it is realm-local -- 416 bytes each, in every realm, even
/// when the spelling is a constant of the runtime. The prelude builds the global object, `process`,
/// `console` and its node objects from C literals written the one way this runtime writes a key:
/// the literal followed by its exact length (`kiln_c99_put(NULL, object, "fd", 2, ...)`), or a
/// table row naming a built-in (`{"parseInt", KILN_C99_METHOD_PARSE_INT}`). Both are recognised
/// here, so a literal added to the prelude joins the process's shared key table at load time with
/// no list to keep. A literal that is not a key (a message fragment with its length) is registered
/// too; that costs one shared-table entry and changes nothing a program can see.
fn prelude_literal_keys(runtime: &Path, launcher: &Path) -> Vec<String> {
    let mut entries = std::fs::read_dir(runtime)
        .expect("read runtime sources")
        .map(|entry| entry.expect("runtime source entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "c" || ext == "h"))
        .collect::<Vec<_>>();
    entries.sort();
    // The graph launcher's C (kiln-native) builds `process.__kilnNativeModules` -- `fs`, `http`,
    // `net`, `ffi` and their members -- in every realm the same way, with this prelude linked.
    if launcher.exists() {
        println!("cargo:rerun-if-changed={}", launcher.display());
        entries.push(launcher.to_path_buf());
    }
    let mut keys = std::collections::BTreeSet::new();
    for path in entries {
        let text = std::fs::read(&path).expect("read runtime source");
        let mut at = 0;
        while at < text.len() {
            if text[at] != b'"' {
                at += 1;
                continue;
            }
            let start = at + 1;
            let mut end = start;
            while end < text.len() && key_byte(text[end], end == start) {
                end += 1;
            }
            if end == start || end >= text.len() || text[end] != b'"' {
                at = end.max(at + 1);
                continue;
            }
            let literal = &text[start..end];
            let mut next = end + 1;
            while next < text.len() && text[next].is_ascii_whitespace() {
                next += 1;
            }
            at = end + 1;
            if next >= text.len() || text[next] != b',' {
                continue;
            }
            next += 1;
            while next < text.len() && text[next].is_ascii_whitespace() {
                next += 1;
            }
            let digits_start = next;
            while next < text.len() && text[next].is_ascii_digit() {
                next += 1;
            }
            let spelled_length = next > digits_start
                && next < text.len()
                && !text[next].is_ascii_alphanumeric()
                && std::str::from_utf8(&text[digits_start..next])
                    .ok()
                    .and_then(|digits| digits.parse::<usize>().ok())
                    == Some(literal.len());
            let table_row = start >= 2
                && text[start - 2] == b'{'
                && text[digits_start..].starts_with(b"KILN_C99_");
            if spelled_length || table_row {
                keys.insert(String::from_utf8(literal.to_vec()).expect("ASCII key"));
            }
        }
    }
    keys.into_iter().collect()
}

fn write_prelude_literal_keys() {
    let runtime = Path::new("src/runtime");
    let keys = prelude_literal_keys(runtime, Path::new("../kiln-native/src/graph.rs"));
    let mut header = String::from(
        "/* GENERATED by crates/native/kiln-backend-c/build.rs from src/runtime and the graph\n \
         * launcher (kiln-native/src/graph.rs): every property key the prelude and the launcher\n \
         * spell as a C literal, registered as a shared key before the first realm\n \
         * (`kiln_c99_register_builtin_keys`). Do not edit. */\n\
         #define KILN_C99_PRELUDE_LITERAL_KEYS(X) \\\n",
    );
    let mut line = String::from("   ");
    for key in &keys {
        let item = format!(" X(\"{key}\")");
        if line.len() + item.len() > 96 {
            header.push_str(&line);
            header.push_str(" \\\n");
            line = String::from("   ");
        }
        line.push_str(&item);
    }
    header.push_str(&line);
    header.push('\n');
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let path = Path::new(&out).join("prelude_literal_keys.h");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(header.as_str()) {
        std::fs::write(&path, header).expect("write prelude_literal_keys.h");
    }
}

fn main() {
    write_prelude_literal_keys();
    let mut bytes = Vec::new();
    fingerprint(Path::new("src"), &mut bytes, None);
    let mut module_emitter_bytes = Vec::new();
    // Runtime C implements helpers declared by the prelude header. It is compiled and linked
    // separately from graph module C and cannot affect the bytes emitted for a ModuleProgram.
    // Keep it in the complete backend identity while excluding it from the module-source cache
    // identity; the prelude/header digest still invalidates dependent object files.
    fingerprint(
        Path::new("src"),
        &mut module_emitter_bytes,
        Some(Path::new("src/runtime")),
    );
    // Emission reads kiln-ir directly -- proto accessors like `effective_name_units`
    // decide emitted bytes without changing the program digest -- so its sources
    // join the fingerprint. Without them an IR-side change reused stale modules:
    // same bytecode in, previous C out.
    fingerprint(Path::new("../../compile/kiln-ir/src"), &mut bytes, None);
    fingerprint(
        Path::new("../../compile/kiln-ir/src"),
        &mut module_emitter_bytes,
        None,
    );
    fingerprint(Path::new("../../compile/kiln-graph/src"), &mut bytes, None);
    fingerprint(
        Path::new("../../compile/kiln-graph/src"),
        &mut module_emitter_bytes,
        None,
    );
    fingerprint(
        Path::new("../../compile/kiln-db/src/summary.rs"),
        &mut bytes,
        None,
    );
    fingerprint(
        Path::new("../../compile/kiln-db/src/summary.rs"),
        &mut module_emitter_bytes,
        None,
    );
    fingerprint(Path::new("Cargo.toml"), &mut bytes, None);
    fingerprint(Path::new("Cargo.toml"), &mut module_emitter_bytes, None);
    fingerprint(Path::new("build.rs"), &mut bytes, None);
    fingerprint(Path::new("build.rs"), &mut module_emitter_bytes, None);
    println!(
        "cargo:rustc-env=KILN_C_EMITTER_BUILD_ID={}",
        kiln_digest::Digest::hash(&bytes).to_hex()
    );
    println!(
        "cargo:rustc-env=KILN_C_MODULE_EMITTER_BUILD_ID={}",
        kiln_digest::Digest::hash(&module_emitter_bytes).to_hex()
    );
}
