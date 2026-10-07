use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "kiln-cli-module-failure-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create fixture");
        // A fixture is its own project, including beneath a temporary-directory `.git` ancestor.
        std::fs::write(root.join("package.json"), "{}\n").expect("write project marker");
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rejects_with_live_resource(setup: &str) {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.0.join("counter.mjs"),
        "import { Actor } from 'kiln:actors'; export class Counter extends Actor {}",
    )
    .expect("write actor module");
    std::fs::write(
        fixture.0.join("main.mjs"),
        format!("{setup}\nawait Promise.reject(new Error('entry failure with live resource'));"),
    )
    .expect("write entry module");
    let mut child = Command::new(env!("CARGO_BIN_EXE_kiln"))
        .current_dir(&fixture.0)
        .args(["--one-shot", "--node", "run", "main.mjs"])
        .env("KILN_JOBS", "2")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run entry module");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().expect("poll kiln").is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("rejected entry remained alive with a referenced resource");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().expect("collect kiln output");
    assert!(
        !output.status.success(),
        "entry rejection must fail the run"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("entry failure with live resource"),
        "original rejection was lost: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejected_entry_reports_without_waiting_for_a_live_actor() {
    rejects_with_live_resource(
        "import { spawnActor } from 'kiln:actors';\n\
         const owner = await spawnActor({module: new URL('./counter.mjs', import.meta.url), export: 'Counter'});",
    );
}

#[test]
fn rejected_entry_reports_without_waiting_for_a_live_interval() {
    rejects_with_live_resource("setInterval(() => {}, 1000);");
}
