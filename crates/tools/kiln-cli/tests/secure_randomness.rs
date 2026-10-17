//! Random values a program uses for secrets must not repeat between runs.
//!
//! Until v0.1.0's release audit the realm's entropy defaulted to ADR 0026's fixed seed in every
//! mode, and `node:crypto` built `randomBytes`, `randomUUID` and `getRandomValues` on
//! `Math.random`, so every run of every program printed the same "random" UUID, session token and
//! key. Two processes are needed to see that, which is why this is a CLI test and not a probe:
//! the conformance corpora compare one run of each engine against the other.
//!
//! The rule held here: on the real clock (`kiln run`'s default) everything differs between runs;
//! `--clock virtual` keeps `Math.random` reproducible, but the crypto functions draw from the
//! operating system in every mode.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

const PROGRAM: &str = r#"
import { randomBytes, randomUUID, getRandomValues, randomFillSync } from 'node:crypto';
const web = new Uint8Array(16); crypto.getRandomValues(web);
const node = new Uint8Array(16); getRandomValues(node);
console.log(JSON.stringify({
  webUuid: crypto.randomUUID(),
  nodeUuid: randomUUID(),
  bytes: randomBytes(16).toString('hex'),
  web: Buffer.from(web).toString('hex'),
  node: Buffer.from(node).toString('hex'),
  fill: randomFillSync(Buffer.alloc(16)).toString('hex'),
  math: Math.random(),
}));
"#;

fn fixture() -> PathBuf {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "kiln-cli-secure-randomness-{}-{id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("create fixture");
    std::fs::write(root.join("package.json"), "{}\n").expect("write project marker");
    std::fs::write(root.join("main.mjs"), PROGRAM).expect("write program");
    root.canonicalize().expect("canonical fixture")
}

/// One interpreter run's output fields, by name.
fn run(root: &PathBuf, extra: &[&str]) -> serde_json::Map<String, serde_json::Value> {
    let output = Command::new(env!("CARGO_BIN_EXE_kiln"))
        .current_dir(root)
        .args(["--one-shot", "run"])
        .args(extra)
        .arg("main.mjs")
        .env("KILN_JOBS", "2")
        .output()
        .expect("run kiln");
    assert!(
        output.status.success(),
        "kiln run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let line = String::from_utf8_lossy(&output.stdout);
    match serde_json::from_str(line.trim()).expect("program prints one JSON object") {
        serde_json::Value::Object(fields) => fields,
        other => panic!("expected an object, got {other}"),
    }
}

const CRYPTO_FIELDS: [&str; 6] = ["webUuid", "nodeUuid", "bytes", "web", "node", "fill"];

#[test]
fn every_random_source_differs_between_runs_on_the_real_clock() {
    let root = fixture();
    let (first, second) = (run(&root, &[]), run(&root, &[]));
    for field in CRYPTO_FIELDS.iter().chain(&["math"]) {
        assert_ne!(
            first[*field], second[*field],
            "`{field}` repeated between two runs: it is predictable"
        );
    }
    for field in ["webUuid", "nodeUuid"] {
        let uuid = first[field].as_str().expect("a UUID string");
        assert_eq!(uuid.len(), 36, "{uuid}");
        assert_eq!(&uuid[14..15], "4", "version 4: {uuid}");
        assert!("89ab".contains(&uuid[19..20]), "RFC 4122 variant: {uuid}");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_virtual_clock_keeps_math_random_reproducible_but_never_crypto() {
    let root = fixture();
    let virtual_clock = ["--clock", "virtual"];
    let (first, second) = (run(&root, &virtual_clock), run(&root, &virtual_clock));
    assert_eq!(
        first["math"], second["math"],
        "Math.random is reproducible on the virtual clock"
    );
    for field in CRYPTO_FIELDS {
        assert_ne!(
            first[field], second[field],
            "`{field}` repeated on the virtual clock: crypto randomness must never be seeded"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}
