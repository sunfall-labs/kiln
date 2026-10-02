//! `spawnActor({ module })` in the interpreter loads any module the name reaches (ADR 0072).
//!
//! The compiler links the actor modules it can see named; the interpreter must not depend on
//! that. These programs name their targets in ways discovery cannot follow -- computed at run
//! time -- or by absolute names that differ per machine, so they live here rather than in the
//! native parity corpus, where the native build is expected to refuse the computed ones.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "kiln-cli-actor-modules-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create fixture");
        Self(root.canonicalize().expect("canonical fixture"))
    }

    fn write(&self, name: &str, source: &str) {
        std::fs::write(self.0.join(name), source).expect("write fixture module");
    }

    fn run(&self, entry: &str) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_kiln"))
            .current_dir(&self.0)
            .args(["--one-shot", "--node", "run", entry])
            .env("KILN_JOBS", "2")
            .output()
            .expect("run kiln");
        assert!(
            output.status.success(),
            "kiln run failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const COUNTER: &str = "import { Actor } from 'kiln:actors';\n\
    export class Counter extends Actor {\n\
      constructor(n) { super(); this.n = n; }\n\
      increment(by) { this.n += by; return this.n; }\n\
      async helper() { return (await import('./helper.mjs')).answer; }\n\
    }\n";

#[test]
fn interpreter_actor_loads_modules_discovery_never_saw() {
    let fixture = Fixture::new();
    for name in ["computed", "absolute", "url"] {
        fixture.write(&format!("counter-{name}.mjs"), COUNTER);
    }
    fixture.write("helper.mjs", "export const answer = 42;\n");
    let absolute = fixture.0.join("counter-absolute.mjs");
    let url = format!("file://{}", fixture.0.join("counter-url.mjs").display());
    fixture.write(
        "main.mjs",
        &format!(
            "import {{ spawnActor }} from 'kiln:actors';\n\
             const show = async (label, owner) => {{\n\
               console.log(label, await owner.ref.call('increment', [1]), await owner.ref.call('helper', []));\n\
               await owner.stop();\n\
             }};\n\
             const name = ['counter-', 'computed'].join('') + '.mjs';\n\
             await show('computed', await spawnActor({{ module: new URL('./' + name, import.meta.url), export: 'Counter', args: [1] }}));\n\
             const names = [{absolute:?}, {url:?}];\n\
             for (const module of names) await show(module.endsWith('url.mjs') ? 'url' : 'absolute', await spawnActor({{ module, export: 'Counter', args: [2] }}));\n\
             try {{ await spawnActor({{ module: new URL('./' + 'missing.mjs', import.meta.url), export: 'Counter' }}); }}\n\
             catch (error) {{ console.log('missing', error.name, error.message.includes('missing.mjs')); }}\n",
        ),
    );
    assert_eq!(
        fixture.run("main.mjs"),
        "computed 2 42\nabsolute 3 42\nurl 3 42\nmissing TypeError true\n"
    );
}

#[test]
fn interpreter_worker_loads_a_script_discovery_never_saw() {
    let fixture = Fixture::new();
    fixture.write(
        "computed.mjs",
        "import { parentPort } from 'node:worker_threads'; parentPort.postMessage('computed ran');\n",
    );
    fixture.write(
        "main.mjs",
        "import { Worker } from 'node:worker_threads';\n\
         const name = ['comp', 'uted.mjs'].join('');\n\
         const worker = new Worker(new URL('./' + name, import.meta.url));\n\
         worker.on('message', (m) => console.log(m));\n\
         worker.on('exit', (code) => console.log('exit', code));\n",
    );
    assert_eq!(fixture.run("main.mjs"), "computed ran\nexit 0\n");
}
