//! Guards for `scripts/`, which is the part of the repository that no compiler checks.
//!
//! A shell script's mistakes surface only when the script runs, and these scripts run at the end
//! of a long CI job or a long local build. `scripts/check-native-number-ffi.sh` shipped for
//! several commits with `env VAR=… kiln_cargo`, which cannot work — `env` execs a *binary* and a
//! shell function is not one — so the script exited 127 at the last step every time. That is a
//! whole class of error the Rust test suite can catch in milliseconds, so it does.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root is reachable from the crate manifest")
}

fn scripts_directory() -> PathBuf {
    repo_root().join("scripts")
}

/// Every `scripts/*.sh`, sorted, so a failure names the same file on every machine.
fn shell_scripts() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(scripts_directory())
        .expect("scripts/ exists")
        .map(|entry| entry.expect("scripts/ is readable").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sh"))
        .collect();
    found.sort();
    assert!(
        !found.is_empty(),
        "found no scripts/*.sh — this guard would pass vacuously, which is worse than no guard"
    );
    found
}

/// The library is sourced, never executed, so it is exempt from the per-script rules below.
const LIBRARIES: [&str; 2] = ["lib.sh", "provider-contract.sh"];

fn is_library(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| LIBRARIES.iter().any(|library| name == *library))
}

/// Names defined as shell functions in `scripts/lib.sh`, e.g. `kiln_cargo`.
///
/// Parsed rather than hard-coded: a hard-coded list silently stops covering functions added after
/// it was written, which is the exact failure mode this file exists to prevent.
fn library_function_names() -> BTreeSet<String> {
    let library = std::fs::read_to_string(scripts_directory().join("lib.sh"))
        .expect("scripts/lib.sh is readable");
    let names: BTreeSet<String> = library
        .lines()
        .filter_map(|line| {
            let name = line.strip_suffix("() {")?;
            let valid = !name.is_empty()
                && name
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character == '_');
            valid.then(|| name.to_owned())
        })
        .collect();
    assert!(
        names.contains("kiln_cargo"),
        "parsed no `kiln_cargo` out of scripts/lib.sh, so the function parser has drifted from the \
         file's style and this guard is no longer looking at anything; found {names:?}"
    );
    names
}

/// Commands that replace themselves with a *binary* of the given name. None of them can see a
/// shell function, so naming one after them is always a latent exit-127.
const BINARY_LAUNCHERS: [&str; 6] = ["env", "xargs", "sudo", "nohup", "timeout", "command"];

/// Options of the launchers above that consume the following token, which therefore is not yet
/// the command word. `env -u NAME cmd` and `timeout -s TERM 5 cmd` both need this.
const LAUNCHER_OPTIONS_TAKING_A_VALUE: [&str; 4] = ["-u", "--unset", "-s", "--signal"];

/// Join backslash-continued lines and drop comments, so a call split across five physical lines —
/// which every invocation in these scripts is — is analysed as the one command it actually is.
fn logical_lines(source: &str) -> Vec<(usize, String)> {
    let mut lines = Vec::new();
    let mut pending = String::new();
    let mut start = 1;
    for (index, raw) in source.lines().enumerate() {
        let number = index + 1;
        let trimmed = raw.trim();
        if pending.is_empty() {
            if trimmed.starts_with('#') {
                continue;
            }
            start = number;
        }
        match raw.strip_suffix('\\') {
            Some(head) => {
                pending.push_str(head);
                pending.push(' ');
            }
            None => {
                pending.push_str(raw);
                lines.push((start, std::mem::take(&mut pending)));
            }
        }
    }
    if !pending.is_empty() {
        lines.push((start, pending));
    }
    lines
}

/// The command word a launcher will try to `exec`, if this token sequence starts with a launcher.
///
/// Skips the launcher's own flags and any `VAR=value` assignment prefixes, because those precede
/// the command word rather than being it.
fn launched_command<'a>(tokens: &[&'a str]) -> Option<&'a str> {
    let (launcher, rest) = tokens.split_first()?;
    if !BINARY_LAUNCHERS.contains(launcher) {
        return None;
    }
    // `command -v name` asks whether a name is callable and answers correctly for functions --
    // `scripts/lib.sh` uses exactly that shape. Only the exec forms of `command` are a problem, so
    // do not report the query form; a guard that cries wolf is a guard somebody deletes.
    if *launcher == "command" && rest.iter().any(|token| *token == "-v" || *token == "-V") {
        return None;
    }
    let mut remaining = rest.iter();
    while let Some(token) = remaining.next() {
        if LAUNCHER_OPTIONS_TAKING_A_VALUE.contains(token) {
            remaining.next();
            continue;
        }
        // A bare flag, or an assignment prefix such as `CC=cc`, is not the command word.
        if token.starts_with('-')
            || token
                .split_once('=')
                .is_some_and(|(name, _)| !name.is_empty())
        {
            continue;
        }
        return Some(token);
    }
    None
}

/// `env VAR=… kiln_cargo` and its relatives exit 127. Fail here instead, by name, with the fix.
#[test]
fn no_script_launches_a_shell_function_through_a_binary_launcher() {
    let functions = library_function_names();
    let mut offences = Vec::new();

    for script in shell_scripts() {
        let source = std::fs::read_to_string(&script).expect("script is readable");
        for (line, logical) in logical_lines(&source) {
            let tokens: Vec<&str> = logical.split_whitespace().collect();
            // Scan every position: the launcher may itself follow `if`, `then`, `|` and friends.
            for start in 0..tokens.len() {
                let Some(command) = launched_command(&tokens[start..]) else {
                    continue;
                };
                if functions.contains(command) {
                    offences.push(format!(
                        "  {}:{line} runs `{command}` through `{}` — write `{command}` on its own, \
                         with any `VAR=value` directly in front of it",
                        script.display(),
                        tokens[start],
                    ));
                }
            }
        }
    }

    assert!(
        offences.is_empty(),
        "a shell function is being launched through a command that execs a binary. `env`, `xargs` \
         and friends look up a *binary* of that name, never a function, so this always fails with \
         exit 127 at run time:\n{}\n\nBash applies an assignment prefix to a function call \
         directly, so the launcher is not needed. Functions defined in scripts/lib.sh: {:?}",
        offences.join("\n"),
        functions,
    );
}

/// The provider contract and its ordered source inventory are shared by runtime and scripts.
#[test]
fn the_provider_source_manifest_is_shared_and_complete() {
    let root = repo_root();
    let contract =
        std::fs::read_to_string(root.join("crates/kiln-native-provider-contract/src/lib.rs"))
            .expect("provider contract is readable");
    let packager =
        std::fs::read_to_string(scripts_directory().join("package-native-number-provider.sh"))
            .expect("packager is readable");
    let smoke = std::fs::read_to_string(scripts_directory().join("check-native-number-ffi.sh"))
        .expect("smoke script is readable");
    let manifest_text =
        std::fs::read_to_string(root.join("crates/kiln-native/provider-sources.txt"))
            .expect("provider source manifest is readable");
    let paths: Vec<_> = manifest_text.lines().collect();

    assert!(
        paths.len() > 10,
        "provider source manifest unexpectedly contains only {} paths: {paths:?}",
        paths.len(),
    );
    assert!(
        paths.first() == Some(&"crates/kiln-native/provider-sources.txt"),
        "provider source manifest must hash itself first: {paths:?}",
    );
    let unique = paths.iter().collect::<BTreeSet<_>>();
    assert_eq!(
        unique.len(),
        paths.len(),
        "provider source manifest contains duplicate entries: {paths:?}",
    );
    for path in &paths {
        assert!(
            !path.is_empty(),
            "provider source manifest contains a blank entry"
        );
        assert_eq!(
            *path,
            path.trim(),
            "provider source manifest entry has surrounding whitespace: {path:?}"
        );
        assert!(
            !path.chars().any(char::is_whitespace),
            "provider source manifest entry contains whitespace: {path:?}",
        );
        assert!(
            root.join(path).is_file(),
            "provider source manifest names a missing file: {path}"
        );
    }
    for (name, source, marker) in [
        ("provider contract", contract, "provider-sources.txt"),
        ("packager", packager, "kiln-native-provider-contract"),
        ("smoke script", smoke, "kiln-native-provider-contract"),
    ] {
        assert!(
            source.contains(marker),
            "{name} does not consume the shared provider source manifest",
        );
    }
}

/// `bash -n` parses without running. It costs milliseconds and catches the unbalanced `fi` that
/// would otherwise be found by a CI job twenty minutes in.
#[test]
fn every_script_parses() {
    for script in shell_scripts() {
        let parsed = Command::new("bash")
            .arg("-n")
            .arg(&script)
            .output()
            .expect("bash is available");
        assert!(
            parsed.status.success(),
            "{} is not valid bash:\n{}",
            script.display(),
            String::from_utf8_lossy(&parsed.stderr),
        );
    }
}

/// An executable script that forgets `set -euo pipefail` reports success after a failed step,
/// which is the worst outcome available to a CI script.
#[test]
fn every_executable_script_is_strict_and_sources_the_library() {
    for script in shell_scripts() {
        if is_library(&script) {
            continue;
        }
        let source = std::fs::read_to_string(&script).expect("script is readable");
        assert!(
            source.starts_with("#!/usr/bin/env bash\n"),
            "{} needs `#!/usr/bin/env bash` on its first line",
            script.display(),
        );
        assert!(
            source.contains("\nset -euo pipefail\n"),
            "{} does not `set -euo pipefail`, so a failing step in it would be reported as a pass",
            script.display(),
        );
        // Calling a library function without sourcing the library is the same exit-127 as above,
        // arrived at from the other direction.
        if library_function_names()
            .iter()
            .any(|function| source.contains(function.as_str()))
        {
            assert!(
                source.contains("/lib.sh\""),
                "{} calls a scripts/lib.sh function but never sources scripts/lib.sh",
                script.display(),
            );
        }
    }
}

/// The shared Test262 policy is Python because it is also consumed by the shell entrypoints.
/// Keep its real contract suite in the normal script-hygiene gate, without creating bytecode in
/// the checkout.
#[test]
fn shared_test262_corpus_policy_suite_passes() {
    let root = repo_root();
    let output = Command::new("python3")
        .arg("-B")
        .arg("scripts/test262_corpus_test.py")
        .current_dir(&root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("python3 is available");
    assert!(
        output.status.success(),
        "shared Test262 corpus policy tests failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn gate_result_envelope_contract_suite_passes() {
    let root = repo_root();
    let output = Command::new("python3")
        .args(["-B", "scripts/gate_results_test.py"])
        .current_dir(&root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("python3 is available");
    assert!(
        output.status.success(),
        "gate result contract tests failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[cfg(unix)]
#[test]
fn every_script_has_the_execute_bit_it_needs() {
    use std::os::unix::fs::PermissionsExt;

    for script in shell_scripts() {
        let mode = std::fs::metadata(&script)
            .expect("script is readable")
            .permissions()
            .mode();
        let executable = mode & 0o111 != 0;
        if is_library(&script) {
            assert!(
                !executable,
                "{} is a sourced library and must not be executable, or someone will run it",
                script.display(),
            );
        } else {
            assert!(
                executable,
                "{} is not executable, so `scripts/{}` fails with `permission denied`",
                script.display(),
                script.file_name().unwrap_or_default().to_string_lossy(),
            );
        }
    }
}
