use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    empty_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "kiln-cli-cache-gc-without-toolchain-{}-{id}",
            std::process::id()
        ));
        let empty_path = root.join("empty-path");
        std::fs::create_dir_all(root.join(".kiln/cache/generated-c/aa"))
            .expect("create cache fixture");
        std::fs::create_dir_all(&empty_path).expect("create empty PATH");
        std::fs::write(
            root.join(".kiln/cache/generated-c/aa").join("a".repeat(64)),
            vec![b'x'; 2048],
        )
        .expect("seed cache record");
        Self { root, empty_path }
    }

    fn kiln(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kiln"));
        command
            .current_dir(&self.root)
            .env("PATH", &self.empty_path);
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn run_gc(fixture: &Fixture, one_shot: bool) -> std::process::Output {
    let mut command = fixture.kiln();
    if one_shot {
        command.arg("--one-shot");
    }
    command.args(["cache", "gc", "--dry-run"]);
    let output = command.output().expect("run kiln cache gc");
    if !one_shot {
        let stopped = fixture
            .kiln()
            .args(["daemon", "stop"])
            .output()
            .expect("stop daemon after cache gc");
        assert!(
            stopped.status.success(),
            "daemon stop failed: {}{}",
            String::from_utf8_lossy(&stopped.stdout),
            String::from_utf8_lossy(&stopped.stderr)
        );
    }
    output
}

#[test]
fn cache_gc_works_in_one_shot_and_daemon_modes_without_a_c_compiler() {
    for one_shot in [true, false] {
        let fixture = Fixture::new();
        let output = run_gc(&fixture, one_shot);
        assert!(
            output.status.success(),
            "{} cache gc failed: {}{}",
            if one_shot { "one-shot" } else { "daemon" },
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("dry run: nothing was deleted"),
            "cache gc did not report its dry run: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
