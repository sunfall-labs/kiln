//! ADR 0068 rule 4 and ADR 0072, for the interpreter's static path: with `KILN_EXTENSIONS=0` every
//! `kiln:` module refuses by name with a `TypeError`, whether the import is static or dynamic, and
//! an engine-internal module is not importable by a program at all (extensions on or off).
//!
//! The dynamic and global-surface checks run in both engines as native probes
//! (`conformance/native/kiln-extensions-off-modules.mjs`, `kiln-private-channel.mjs`); a static
//! import that fails ends the program, so its message is read here.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "kiln-cli-extensions-off-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create fixture");
        // `temp_dir()` can live beneath a user's checkout (including a `.git` ancestor). Mark
        // each fixture as its own project so concurrent CLI runs do not discover sibling
        // fixtures as one shared source tree.
        std::fs::write(root.join("package.json"), "{}\n").expect("write project marker");
        Self(root.canonicalize().expect("canonical fixture"))
    }

    fn write(&self, name: &str, source: &str) {
        std::fs::write(self.0.join(name), source).expect("write fixture module");
    }

    /// `(succeeded, stdout, stderr)` of one interpreter run.
    fn run(&self, entry: &str, extensions: bool) -> (bool, String, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kiln"));
        command
            .current_dir(&self.0)
            .args(["--one-shot", "--node", "run", entry])
            .env("KILN_JOBS", "2");
        if !extensions {
            command.env("KILN_EXTENSIONS", "0");
        }
        let output = command.output().expect("run kiln");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn static_import_of_a_kiln_module_names_it_with_a_type_error_when_extensions_are_off() {
    let fixture = Fixture::new();
    for specifier in ["kiln:shared", "kiln:actors", "kiln:process"] {
        let entry = format!("static-{}.mjs", specifier.trim_start_matches("kiln:"));
        fixture.write(
            &entry,
            &format!("import '{specifier}';\nconsole.log('unreachable');\n"),
        );
        let (succeeded, stdout, stderr) = fixture.run(&entry, false);
        assert!(
            !succeeded,
            "{specifier} imported with extensions off: {stdout}"
        );
        assert!(!stdout.contains("unreachable"), "{specifier}: {stdout}");
        assert!(
            stderr.contains(&format!(
                "TypeError: {specifier} requires enabled Kiln extensions"
            )),
            "{specifier}: {stderr}"
        );
    }
}

#[test]
fn dynamic_import_of_a_kiln_module_names_it_with_the_same_type_error() {
    let fixture = Fixture::new();
    fixture.write(
        "dynamic.mjs",
        "for (const load of [() => import('kiln:shared'), () => import('kiln:actors'), () => import('kiln:process')]) {\n\
           try { await load(); console.log('imported'); }\n\
           catch (error) { console.log(error.name, error.message); }\n\
         }\n",
    );
    let (succeeded, stdout, stderr) = fixture.run("dynamic.mjs", false);
    assert!(succeeded, "{stdout}{stderr}");
    assert_eq!(
        stdout,
        "TypeError kiln:shared requires enabled Kiln extensions\n\
         TypeError kiln:actors requires enabled Kiln extensions\n\
         TypeError kiln:process requires enabled Kiln extensions\n"
    );
}

#[test]
fn a_program_cannot_import_an_engine_internal_module() {
    let fixture = Fixture::new();
    fixture.write(
        "core.mjs",
        "import { isolate } from 'kiln:actors/core';\nconsole.log(typeof isolate);\n",
    );
    fixture.write(
        "worker.mjs",
        "import 'kiln:actors/worker';\nconsole.log('imported');\n",
    );
    for extensions in [true, false] {
        for entry in ["core.mjs", "worker.mjs"] {
            let (succeeded, stdout, stderr) = fixture.run(entry, extensions);
            assert!(
                !succeeded,
                "{entry} imported (extensions {extensions}): {stdout}"
            );
            assert!(
                stderr.contains("is internal to kiln:actors"),
                "{entry} (extensions {extensions}): {stderr}"
            );
        }
    }
}

#[test]
fn the_private_channel_is_unreachable_from_a_program() {
    let fixture = Fixture::new();
    fixture.write(
        "private.mjs",
        "import 'node:worker_threads';\n\
         console.log('__kilnPrivate' in globalThis, typeof globalThis.__kilnPrivate);\n\
         console.log(Object.getOwnPropertyNames(globalThis).filter((key) => /^__kiln/.test(key)).length);\n\
         console.log(Object.getOwnPropertySymbols(MessagePort).length, Object.getOwnPropertySymbols(structuredClone).length);\n",
    );
    for extensions in [true, false] {
        let (succeeded, stdout, stderr) = fixture.run("private.mjs", extensions);
        assert!(succeeded, "{stdout}{stderr}");
        assert_eq!(
            stdout, "false undefined\n0\n0 0\n",
            "extensions {extensions}"
        );
    }
}
