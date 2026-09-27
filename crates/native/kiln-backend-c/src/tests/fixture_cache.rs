//! Cache compilation, never execution. Preprocessing is repeated so SDK/system header changes
//! participate in the input, and every test relinks and runs against the current host libraries.
use kiln_artifact_cache::{
    ArtifactCache, ArtifactKey, ArtifactKind, Backend, BuildProfile, Lookup,
};
use kiln_digest::Digest;
use kiln_native_toolchain::{CompilerKind, LinkOptions, Toolchain};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};

const FLAGS: &[&str] = &[
    "-std=c99",
    "-pedantic",
    "-Werror=implicit-function-declaration",
    "-frandom-seed=kiln-backend-fixture",
];
static TOOLCHAIN: OnceLock<Toolchain> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kiln-c-fixture-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).expect("private fixture directory");
        Self(root)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn compiler(toolchain: &Toolchain) -> Command {
    let mut command = Command::new(&toolchain.identity().path);
    if toolchain.identity().kind == CompilerKind::Zig {
        command.arg("cc");
    }
    command.args(FLAGS);
    command
}

fn with_input(command: &mut Command, input: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("fixture compiler starts");
    let mut stdin = child.stdin.take().unwrap();
    // Preprocessor output can exceed a pipe buffer before it has consumed all its input.
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(input));
        let output = child.wait_with_output().expect("fixture compiler exits");
        let written = writer.join().expect("fixture writer joins");
        assert!(
            output.status.success(),
            "fixture compiler failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        written.expect("fixture source is accepted");
        output
    })
}

fn object_key(toolchain: &Toolchain, preprocessed: &[u8]) -> ArtifactKey {
    ArtifactKey::new(
        ArtifactKind::NativeObject,
        toolchain.identity().stable_key(),
        Backend::C,
        "backend-fixture-object-v1",
        &toolchain.identity().target,
        "generic",
        BuildProfile::Check,
        "fixture",
        Digest::hash(format!("{FLAGS:?};-E;-P;-x;c;-c;-x;cpp-output").as_bytes()),
        [Digest::hash(preprocessed)],
    )
}

fn run_cached(source: &str, cache: &ArtifactCache, toolchain: &Toolchain) -> (Output, bool) {
    let directory = Directory::new();
    let object = directory.0.join("fixture.o");
    let executable = directory.0.join("fixture");
    let preprocessed = with_input(
        compiler(toolchain).args(["-E", "-P", "-x", "c", "-"]),
        source.as_bytes(),
    )
    .stdout;
    let key = object_key(toolchain, &preprocessed);
    let hit = match cache.lookup(&key).expect("read verified fixture object") {
        Lookup::Hit(artifact) => {
            std::fs::write(&object, artifact.bytes).expect("materialize fixture object");
            true
        }
        Lookup::Miss => {
            with_input(
                compiler(toolchain)
                    .args(["-c", "-x", "cpp-output", "-", "-o"])
                    .arg(&object),
                &preprocessed,
            );
            cache
                .put(&key, &std::fs::read(&object).expect("read compiled object"))
                .expect("publish fixture object");
            false
        }
    };
    toolchain
        .link_executable(
            &[object],
            &executable,
            &LinkOptions {
                libraries: vec!["m".into()],
                ..Default::default()
            },
        )
        .expect("fixture links against current host libraries");
    (
        Command::new(executable).output().expect("fixture executes"),
        hit,
    )
}

pub(super) fn run(source: &str) -> Output {
    let toolchain = TOOLCHAIN
        .get_or_init(|| Toolchain::discover().expect("C compiler for backend tests (honors CC)"));
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/kiln-backend-test-cache");
    let (output, _) = run_cached(source, &ArtifactCache::new(root), toolchain);
    output
}

#[test]
fn cached_objects_reexecute_and_track_source_headers_and_compiler_identity() {
    let toolchain = Toolchain::discover().unwrap();
    let directory = Directory::new();
    let cache = ArtifactCache::new(directory.0.join("cache"));
    let header = directory.0.join("value.h");
    let executions = directory.0.join("executions");
    std::fs::write(&header, "#define RESULT 11\n").unwrap();
    let source = format!(
        "#include \"{}\"\n#include <stdio.h>\nint main(void) {{ FILE *f = fopen(\"{}\", \"a\"); if (!f) return 99; fputc('x', f); fclose(f); return RESULT; }}",
        header.display(),
        executions.display()
    );
    let (first, hit) = run_cached(&source, &cache, &toolchain);
    assert!(!hit);
    assert_eq!(first.status.code(), Some(11));
    let (warm, hit) = run_cached(
        &source,
        &ArtifactCache::new(directory.0.join("cache")),
        &toolchain,
    );
    assert!(hit);
    assert_eq!(warm.status.code(), Some(11));
    assert_eq!(std::fs::read(&executions).unwrap(), b"xx");
    std::fs::write(&header, "#define RESULT 12\n").unwrap();
    let (changed, hit) = run_cached(&source, &cache, &toolchain);
    assert!(!hit);
    assert_eq!(changed.status.code(), Some(12));
    let (changed, hit) = run_cached("int main(void) { return 13; }", &cache, &toolchain);
    assert!(!hit);
    assert_eq!(changed.status.code(), Some(13));
    let key = object_key(&toolchain, b"same source");
    let mut different_compiler = key.clone();
    different_compiler.compiler_version.push_str("-changed");
    assert_ne!(key.digest(), different_compiler.digest());
}

#[allow(dead_code)]
mod emission_fingerprint {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs"));

    #[test]
    fn fixture_edits_do_not_invalidate_emission_but_production_edits_do() {
        let directory = super::Directory::new();
        let source = directory.0.join("src");
        std::fs::create_dir_all(source.join("tests")).unwrap();
        std::fs::write(source.join("emitter.rs"), "production").unwrap();
        std::fs::write(source.join("tests.rs"), "tests").unwrap();
        std::fs::write(source.join("tests/fixture.rs"), "fixture").unwrap();
        let mut before = Vec::new();
        fingerprint(&source, &mut before);
        std::fs::write(source.join("tests.rs"), "changed test module").unwrap();
        std::fs::write(source.join("tests/fixture.rs"), "changed fixture").unwrap();
        let mut after = Vec::new();
        fingerprint(&source, &mut after);
        assert_eq!(before, after);
        std::fs::write(source.join("emitter.rs"), "changed production emission").unwrap();
        after.clear();
        fingerprint(&source, &mut after);
        assert_ne!(before, after);
    }
}
