use super::*;

#[test]
fn inherited_classes_are_not_removable_declarations() {
    for source in [
        "class Unused extends Later {} class Later {}",
        "class Base {} Base = 3; class Unused extends Base {}",
        "class Base {} const Unused = class extends Base {};",
    ] {
        let module =
            kiln_ir::compile_module(&kiln_frontend_oxc::parse("main.mjs", source)).unwrap();
        assert_eq!(
            module.link_facts.removable_declarations.len(),
            1,
            "only the class without heritage is removable: {source}"
        );
    }
}

#[test]
fn native_unhandled_top_level_rejection_exits_with_a_live_handle() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native rejection fixture: no C compiler found");
        return;
    };
    let project = TestDirectory::new("native-unhandled-top-level-rejection-live-handle");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "setInterval(() => {}, 50);\nconsole.log(\"armed\");\nawait Promise.reject(new TypeError(\"unhandled on purpose\"));\n",
    )
    .expect("write fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("fixture compiles");
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("fixture builds");
    for scheduler in [None, Some(false), Some(true)] {
        let mut command = Command::new(&report.output);
        let realm_output = project.path.join(format!("realms-{scheduler:?}"));
        if let Some(scheduler) = scheduler {
            fs::create_dir(&realm_output).expect("create realm output directory");
            command
                .env("KILN_REALMS", "2")
                .env("KILN_REALMS_OUT", &realm_output);
            if scheduler {
                command.env("KILN_REALM_SCHEDULER", "1");
            }
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn_when_launchable()
            .expect("spawn fixture");
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll fixture") {
                break status;
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the fixture is still running after 20s: an unhandled rejection with a live timer hung"
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        let output = child.wait_with_output().expect("collect fixture output");
        assert_eq!(
            status.code(),
            Some(1),
            "uncaught rejection must exit cleanly: {status:?}"
        );
        if scheduler.is_some() {
            for index in 0..2 {
                let output = fs::read_to_string(realm_output.join(format!("{index}.out")))
                    .expect("read realm output");
                let status = fs::read_to_string(realm_output.join(format!("{index}.status")))
                    .expect("read realm status");
                assert_eq!(status.trim(), "1");
                assert!(output.contains("unhandled on purpose"), "{output}");
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("unhandled on purpose"), "{stderr}");
        }
    }
}

#[test]
fn native_unsettled_top_level_await_warns_and_exits_13() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::process::Command;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native unsettled top-level await fixture: no C compiler found");
        return;
    };
    let project = TestDirectory::new("native-unsettled-top-level-await");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "console.log(\"before\");\nawait new Promise(() => {});\nconsole.log(\"never\");\n",
    )
    .expect("write fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("fixture compiles");
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("fixture builds");
    let output = Command::new(&report.output)
        .output_when_launchable()
        .expect("run fixture");
    // Node's answer, and the interpreter's (issue 09 of `docs/issues/full-runtime`): the warning
    // and status 13, which is the one status `main` does not fold to 1.
    assert_eq!(output.status.code(), Some(13));
    assert_eq!(output.stdout, b"before\n");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "Warning: Detected unsettled top-level await"
    );
}

#[test]
fn native_timer_callbacks_retain_closures_through_gc_and_interval_cancel() {
    let source = r#"
function scheduleFactory(value) {
  const captured = { value };
  const delay = {
    valueOf() {
      const garbage = [];
      for (let i = 0; i < 5000; i++) garbage.push({ i, text: 'allocation' });
      return 0;
    }
  };
  const observed = [];
  setTimeout(() => {
    observed.push(captured.value);
    setTimeout(() => observed.push(captured.value + 1), 0);
  }, delay);
  let ticks = 0;
  const interval = setInterval(() => {
    ticks++;
    if (ticks === 2) {
      clearInterval(interval);
      setTimeout(() => console.log(observed.join(',') + ':' + ticks + ':' + captured.value), 0);
    }
  }, 0);
}
scheduleFactory(41);
"#;
    let Some(output) = run_native_release_fixture("native-timer-gc-liveness", source) else {
        return;
    };
    assert_eq!(output, "41,42:2:41\n");
}

#[test]
fn native_release_date_setters_preserve_actual_optional_arguments() {
    let source = r#"
const omitted = new Date(Date.UTC(2024, 6, 15));
omitted.setUTCFullYear(2025);
const present = new Date(Date.UTC(2024, 6, 15));
present.setUTCFullYear(2025, 11, 25);
const explicitUndefined = new Date(Date.UTC(2024, 6, 15));
explicitUndefined.setUTCFullYear(2025, undefined, 25);
const called = new Date(Date.UTC(2024, 6, 15));
Date.prototype.setUTCFullYear.call(called, 2025, 11, 25);
const reflected = new Date(Date.UTC(2024, 6, 15));
Reflect.apply(Date.prototype.setUTCFullYear, reflected, [2025, 11, 25]);
const hours = new Date(Date.UTC(2024, 2, 15, 23));
hours.setUTCHours(23, 59, 59, 999);
console.log([
  omitted.toISOString(),
  present.toISOString(),
  Number.isNaN(explicitUndefined.getTime()),
  called.toISOString(),
  reflected.toISOString(),
  hours.toISOString(),
].join("\n"));
"#;
    let Some(output) = run_native_release_fixture("native-release-date-setter-arguments", source)
    else {
        return;
    };
    assert_eq!(
        output,
        concat!(
            "2025-07-15T00:00:00.000Z\n",
            "2025-12-25T00:00:00.000Z\n",
            "true\n",
            "2025-12-25T00:00:00.000Z\n",
            "2025-12-25T00:00:00.000Z\n",
            "2024-03-15T23:59:59.999Z\n",
        )
    );
}

#[test]
fn native_non_callable_diagnostic_keeps_call_site_and_callee_context() {
    let Some(stderr) = run_native_failure_fixture(
        "kiln-native-non-callable-diagnostic",
        "function invoke() { const value = {}; value(); }\ninvoke();\n",
    ) else {
        return;
    };
    assert!(
        stderr.contains("module=main.mjs"),
        "missing module: {stderr}"
    );
    assert!(
        stderr.contains("operation=Call"),
        "missing operation: {stderr}"
    );
    assert!(
        stderr.contains("detail=non-callable callee kind"),
        "missing value kind: {stderr}"
    );
    assert!(
        stderr.contains("function=invoke"),
        "missing owning function: {stderr}"
    );
    assert!(
        stderr.contains("receiver_tag=9"),
        "missing offending value tag: {stderr}"
    );
}

#[test]
fn native_non_callable_caught_error_does_not_leak_into_later_throw() {
    let Some(stderr) = run_native_failure_fixture(
        "kiln-native-non-callable-diagnostic-reset",
        "function caught() { try { const value = {}; value(); } catch (_) {} }\nfunction boom() { throw new Error('boom'); }\ncaught();\nboom();\n",
    ) else {
        return;
    };
    assert!(
        stderr.contains("detail=Call invocation threw"),
        "stale non-callable detail: {stderr}"
    );
    assert!(
        !stderr.contains("function=caught"),
        "caught context leaked: {stderr}"
    );
}

#[test]
fn native_non_callable_nested_error_names_innermost_function() {
    let Some(stderr) = run_native_failure_fixture(
        "kiln-native-non-callable-nested-function",
        "function outer() { function inner() { const value = {}; value(); } inner(); }\nouter();\n",
    ) else {
        return;
    };
    assert!(
        stderr.contains("function=inner"),
        "missing innermost owning function: {stderr}"
    );
    assert!(
        !stderr.contains("function=outer"),
        "enclosing function leaked into diagnostic: {stderr}"
    );
}

#[test]
fn native_non_callable_cross_module_error_keeps_innermost_site() {
    let Some(stderr) = run_native_failure_fixture_with_dependency(
        "kiln-native-non-callable-cross-module",
        "import { outer } from './dep.mjs';\nouter();\n",
        "export function outer() { function inner() { const value = {}; value(); } inner(); }\n",
        NativeProfile::Dev,
    ) else {
        return;
    };
    assert!(
        stderr.contains("operation=Call"),
        "missing operation: {stderr}"
    );
    assert!(
        stderr.contains("module=dep.mjs"),
        "missing innermost module: {stderr}"
    );
    assert!(
        stderr.contains("function=inner"),
        "missing innermost function: {stderr}"
    );
    assert!(
        !stderr.contains("function=outer"),
        "outer function leaked: {stderr}"
    );

    let Some(stderr) = run_native_failure_fixture_with_dependency(
        "kiln-native-non-callable-cross-module-release",
        "import { outer } from './dep.mjs';\nouter();\n",
        "export function outer() { function inner() { const value = {}; value(); } inner(); }\n",
        NativeProfile::Release,
    ) else {
        return;
    };
    assert!(
        stderr.contains("module=dep.mjs"),
        "release missing innermost module: {stderr}"
    );
    assert!(
        stderr.contains("operation=Call"),
        "release missing operation: {stderr}"
    );
    assert!(
        stderr.contains("detail=non-callable callee kind"),
        "release missing refusal reason: {stderr}"
    );
    assert!(
        !stderr.contains("unknown native operation"),
        "release lost diagnostic provenance: {stderr}"
    );
}

#[test]
fn owned_result_wrapper_source_is_deterministic_and_has_no_static_result_state() {
    let source = owned_result_wrapper_source(
        "kiln_probe",
        "scalar",
        "kiln_probe(&proxy_host, &value)",
        "",
    );
    assert_eq!(
        source,
        owned_result_wrapper_source(
            "kiln_probe",
            "scalar",
            "kiln_probe(&proxy_host, &value)",
            "",
        )
    );
    for fragment in [
        "kiln_probe_api_v2",
        "kiln_module_api_v2",
        "kiln_result_v2",
        "kiln_v2_scalar_kiln_probe_run",
        "kiln_v2_scalar_kiln_probe_read",
        "kiln_v2_scalar_kiln_probe_dispose",
        "failure_status",
        "*out = NULL",
        "memcpy((void *)copy->data, src_str->data, unit_bytes)",
    ] {
        assert!(
            source.contains(fragment),
            "missing generated fragment: {fragment}"
        );
    }
    assert!(!source.contains("static kiln_result_v2"));
    assert!(!source.contains("static kiln_value result"));
}

#[test]
fn largest_first_compilation_preserves_results_and_error_precedence() {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let observed = parking_lot::Mutex::new(Vec::new());
    let result = pool
        .install(|| {
            compile_largest_first(&[1, 20, 3, 20], "priority test", |index| {
                observed.lock().push(index);
                Ok(index)
            })
        })
        .unwrap();
    assert_eq!(*observed.lock(), vec![1, 3, 2, 0]);
    assert_eq!(result, vec![0, 1, 2, 3]);
    let failure = pool
        .install(|| {
            compile_largest_first(&[1, 20, 3, 20], "priority test", |index| {
                if index == 0 || index == 3 {
                    return Err(NativeError::NumberProvider {
                        message: format!("index {index}"),
                    });
                }
                Ok(index)
            })
        })
        .unwrap_err();
    assert!(matches!(failure, NativeError::NumberProvider { message } if message == "index 0"));
    let failure = compile_largest_first(&[1, 20, 3], "priority panic", |index| {
        assert_ne!(index, 1, "deliberate test panic");
        Ok(index)
    })
    .unwrap_err();
    assert!(matches!(
        failure,
        NativeError::WorkerPanic {
            stage: "priority panic"
        }
    ));
}

#[test]
fn one_slow_index_does_not_hold_the_indices_behind_it() {
    let workers = rayon::current_num_threads();
    if workers < 2 {
        // Nothing to steal from on a single-core machine.
        return;
    }
    let count = workers * 4;
    // Index zero blocks until every other index has finished. Under a static partition its
    // chunk-mates could never run, so this would time out; claiming indices one at a time lets
    // the other workers drain the whole rest of the space while zero waits.
    let progress = (std::sync::Mutex::new(0usize), std::sync::Condvar::new());
    let progress_ref = &progress;
    let observed = compile_indexed(count, "test compile", move |index| {
        let (finished, signal) = progress_ref;
        if index == 0 {
            let mut finished = finished.lock().expect("progress lock");
            while *finished < count - 1 {
                let (guard, timeout) = signal
                    .wait_timeout(finished, Duration::from_secs(20))
                    .expect("progress lock");
                finished = guard;
                if timeout.timed_out() {
                    break;
                }
            }
            return Ok(*finished);
        }
        *finished.lock().expect("progress lock") += 1;
        signal.notify_all();
        Ok(0)
    })
    .expect("every index compiles");
    assert_eq!(
        observed[0],
        count - 1,
        "every other index should finish while index zero is blocked"
    );
}

#[test]
fn async_realm_contract_is_in_runtime_not_application_identity() {
    // Shape, not value. This was a pinned literal, and on 2026-08-31 alone it went 3 -> 5
    // -> 15 -> 17: three intended bumps, three red baselines, and not one accidental bump
    // caught. A tripwire that fires only on the change it is meant to permit is friction,
    // and the repair -- editing the literal -- teaches nothing and is indistinguishable
    // from rubber-stamping a mistake.
    //
    // What actually protects the caches is that the identity *has* every component, since a
    // missing one silently makes two different ABIs look alike. A bump is then visible in
    // the diff of the constant it comes from, where a reviewer can see what moved.
    let identity = module_abi_identity();
    for component in ["host-", "c-", "generator-"] {
        assert!(
            identity.contains(component),
            "module ABI identity {identity:?} has no `{component}` component; a missing one \
                 makes two different ABIs hash alike and every cached artifact is then suspect"
        );
    }
    assert!(runtime_abi_identity().contains("async-6"));
    // Same reasoning as above, and the same history: this pin was 14, then 15, then 18, and
    // is 23 as this is written -- all intended, none caught by being pinned.
    // What matters is that the provider's version reaches the identity at all: if it did not,
    // an archive built before an ABI change would keep being linked against emitted C that
    // expects the change, which is exactly the "undefined symbols from a stale archive"
    // failure this repository loses time to.
    let provider = native_number_abi_identity();
    let version = provider
        .rsplit_once('-')
        .map(|(_, version)| version)
        .unwrap_or_default();
    assert!(
        provider.starts_with("kiln-native-number-provider-") && version.parse::<u32>().is_ok(),
        "provider identity {provider:?} must end in the ABI version, or a stale archive and \
             a fresh one hash alike"
    );
}

#[test]
fn successful_private_runs_require_one_well_formed_scalar_result() {
    let root = TestDirectory::new("kiln-native-result");
    let result_path = root.path.join("result.bin");

    // An absent file after a successful run is `None`, not an error. The result carries only
    // the five boundary scalar tags, and a module's completion value is under no obligation to
    // be one of them -- a `--node` entry completes with its `module.exports`, an object. The
    // launcher declines to write what it cannot represent, and declining is not failing: it
    // used to charge that refusal to the program's exit status, so a program that printed
    // correctly exited 1. Unrepresentable is absent, which is what the `Option` already meant.
    assert_eq!(decode_run_result(&result_path, true).unwrap(), None);
    assert_eq!(decode_run_result(&result_path, false).unwrap(), None);

    fs::write(&result_path, b"not-a-result").expect("write malformed result");
    assert!(matches!(
        decode_run_result(&result_path, true),
        Err(NativeError::RunResult { .. })
    ));

    let expected = HostValue::String("line one\nline two\0".into());
    fs::write(
        &result_path,
        kiln_native_runtime::encode_native_result(&expected),
    )
    .expect("write valid result");
    assert_eq!(
        decode_run_result(&result_path, true).expect("decode valid result"),
        Some(expected)
    );
}

#[test]
fn shared_publication_serializes_each_header_library_pair() {
    let root = TestDirectory::new("kiln-native-shared-publication");
    let output = root.path.join("program.so");
    let loader = root.path.join("kiln-content.so");
    let header = shared_library_header_path(&output);
    materialize(&header, b"old header", false).expect("publish old header");
    materialize(&output, b"old library", false).expect("publish old library");

    let process_lock = SHARED_PUBLICATION_LOCK.lock();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let writer_output = output.clone();
    let writer_loader = loader.clone();
    let writer = std::thread::spawn(move || {
        entered_tx.send(()).expect("announce waiting writer");
        let result = publish_shared_artifacts(
            &writer_output,
            &writer_loader,
            b"new header",
            b"new library",
        );
        finished_tx.send(result).expect("report publication");
    });
    entered_rx.recv().expect("writer starts");
    assert!(
        finished_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "a second publisher must wait for the pair lock"
    );
    assert_eq!(fs::read(&header).expect("read old header"), b"old header");
    assert_eq!(fs::read(&output).expect("read old library"), b"old library");

    drop(process_lock);
    finished_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("writer completes after unlock")
        .expect("publication succeeds");
    writer.join().expect("writer exits");
    assert_eq!(fs::read(&header).expect("read new header"), b"new header");
    assert_eq!(fs::read(&output).expect("read new library"), b"new library");
    assert_eq!(
        fs::read(&loader).expect("read loader library"),
        b"new library"
    );

    let collision = publish_shared_artifacts(&output, &loader, b"other header", b"other library")
        .expect_err("a canonical content name is immutable");
    assert!(matches!(
        collision,
        NativeError::ImmutableArtifactCollision { .. }
    ));
    assert_eq!(fs::read(header).unwrap(), b"new header");
    assert_eq!(fs::read(&output).unwrap(), b"new library");

    #[cfg(unix)]
    {
        fs::remove_file(&loader).expect("remove regular loader");
        std::os::unix::fs::symlink(&output, &loader).expect("create matching loader symlink");
        let error = publish_shared_artifacts(&output, &loader, b"new header", b"new library")
            .expect_err("a symlink cannot stand in for an immutable loader");
        assert!(matches!(
            error,
            NativeError::ImmutableArtifactCollision { .. }
        ));
        assert!(
            fs::symlink_metadata(&loader)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

#[test]
fn native_process_argv_reports_the_single_executable_command_line() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native process.argv integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-process-argv");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"console.log(JSON.stringify([
  process.argv.length,
  process.argv[0] === process.argv[1],
  process.execPath === process.argv[0],
  process.argv.slice(2),
]));
"#,
    )
    .expect("write process.argv fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("process.argv source compiles");
    let output = project.path.join("program");
    NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: output.clone(),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("process.argv graph builds natively");

    let bare = std::process::Command::new(&output)
        .output_when_launchable()
        .expect("run the compiled program");
    assert_eq!(bare.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&bare.stdout),
        "[2,true,true,[]]\n",
        "stderr: {}",
        String::from_utf8_lossy(&bare.stderr)
    );

    let passed = std::process::Command::new(&output)
        .args(["a", "b", "c"])
        .output_when_launchable()
        .expect("run the compiled program with arguments");
    assert_eq!(passed.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&passed.stdout),
        "[5,true,true,[\"a\",\"b\",\"c\"]]\n",
        "stderr: {}",
        String::from_utf8_lossy(&passed.stderr)
    );
}

#[test]
fn native_process_next_tick_uses_the_realm_lane_before_microtasks() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native process.nextTick integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-process-next-tick");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"const order = [];
const nextTick = process.nextTick;
order.push(`${delete process.nextTick}:${process.nextTick === undefined}`);
Promise.resolve().then(function () { order.push("promise"); });
nextTick(function (first, second) {
  order.push(`${first}:${second}:${this === undefined}`);
  nextTick(function () { order.push("nested"); });
  queueMicrotask(function () { order.push("micro-from-tick"); });
}, "a", 2);
queueMicrotask(function () { order.push("micro"); });
setTimeout(function () { console.log(order.join(",")); }, 0);
"#,
    )
    .expect("write process.nextTick fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("process.nextTick source compiles");
    let run = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("process.nextTick graph runs natively");
    assert_eq!(run.exit_code, Some(0));
    assert_eq!(
        run.stdout,
        // An ES module body runs inside a promise job, so the promise queue it filled drains
        // before its ticks (node prints the same); a CommonJS body would put the ticks first.
        b"true:true,promise,micro,a:2:true,nested,micro-from-tick\n"
    );
    assert!(run.stderr.is_empty());
}

#[test]
fn native_runtime_callable_properties_are_realm_wide_across_modules() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-realm-callable-properties");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
        .expect("write package metadata");
    fs::write(
        root.path.join("writer.mjs"),
        r#"import { Buffer } from "node:buffer";
Buffer.realmMarker = { answer: 42 };
export { Buffer };"#,
    )
    .expect("write callable-property producer");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import { Buffer as written } from "./writer.mjs";
import { Buffer as read } from "node:buffer";
console.log(written === read, read.realmMarker.answer);"#,
    )
    .expect("write callable-property consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build realm callable-property graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping realm callable-property integration: no C compiler found");
        return;
    };
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native realm callable-property graph builds and runs");
    assert_eq!(
        report.exit_code,
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&report.stdout),
        String::from_utf8_lossy(&report.stderr)
    );
    assert_eq!(report.stdout, b"true 42\n");
}

#[test]
fn native_realm_singletons_are_shared_across_generated_modules() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-realm-singletons");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
        .expect("write package metadata");
    fs::write(
        root.path.join("writer.mjs"),
        r#"globalThis.realmMarker = 41;
process.realmMarker = 42;
Atomics.realmMarker = 43;
export const ready = true;"#,
    )
    .expect("write singleton producer");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import { ready } from "./writer.mjs";
console.log(ready, globalThis.realmMarker, process.realmMarker, Atomics.realmMarker);"#,
    )
    .expect("write singleton consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build realm-singleton graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping realm-singleton integration: no C compiler found");
        return;
    };
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native realm-singleton graph builds and runs");
    assert_eq!(
        report.exit_code,
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&report.stdout),
        String::from_utf8_lossy(&report.stderr)
    );
    assert_eq!(report.stdout, b"true 41 42 43\n");
}

#[test]
fn native_static_unsupported_builtin_remains_fatal() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-static-unsupported-builtin");
    let entry = root.path.join("main.mjs");
    fs::write(&entry, "import 'node:zlib';\n").expect("write static zlib entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build static zlib graph");
    let reject_zlib = |graph: &ModuleGraph, node: &kiln_module_graph::ModuleNode| {
        if node.source == ModuleSource::NodeBuiltin(kiln_module_resolver::NodeBuiltin::Zlib) {
            return Err(NativeError::UnsupportedNodeBuiltin {
                specifier: "node:zlib",
            });
        }
        validate_native_builtin_node(graph, node)
    };
    let error = native_graph_plan_with_validator(&graph, &reject_zlib)
        .expect_err("policy-rejected zlib must remain fatal");
    assert!(matches!(
        error,
        NativeError::UnsupportedNodeBuiltin {
            specifier: "node:zlib"
        }
    ));
}

#[test]
fn native_runtime_unsupported_builtin_is_deferred_and_excluded() {
    use kiln_module_resolver::{NodeBuiltin, NodeRuntimeResolver};

    let root = TestDirectory::new("kiln-native-runtime-unsupported");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "import { createRequire } from 'node:module'; createRequire(import.meta.url)('node:zlib');\n",
    )
    .expect("write runtime builtin entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build runtime builtin graph");
    let reject_zlib = |graph: &ModuleGraph, node: &kiln_module_graph::ModuleNode| {
        if node.source == ModuleSource::NodeBuiltin(kiln_module_resolver::NodeBuiltin::Zlib) {
            return Err(NativeError::UnsupportedNodeBuiltin {
                specifier: "node:zlib",
            });
        }
        validate_native_builtin_node(graph, node)
    };
    let plan = native_graph_plan_with_validator(&graph, &reject_zlib)
        .expect("policy-rejected runtime builtin is deferred");
    let zlib = graph
        .modules()
        .find(|node| node.source == ModuleSource::NodeBuiltin(NodeBuiltin::Zlib))
        .expect("zlib runtime target");
    assert!(plan.excluded.contains(&zlib.path));
    assert!(
        plan.runtime_failures
            .keys()
            .any(|(owner, _, specifier)| { owner == graph.entry() && specifier == "node:zlib" })
    );
}

#[test]
fn runtime_alias_targets_remain_linked_without_eager_evaluation() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-runtime-alias-liveness");
    let entry = root.path.join("main.ts");
    fs::write(
        root.path.join("package.json"),
        r#"{"type":"module","sideEffects":false}"#,
    )
    .expect("write side-effect metadata");
    fs::write(
        root.path.join("tsconfig.json"),
        r#"{"compilerOptions":{"baseUrl":".","paths":{"runtime/*":["src/*.ts"]}}}"#,
    )
    .expect("write project path alias");
    fs::create_dir_all(root.path.join("src")).expect("create source directory");
    fs::write(
        root.path.join("src/compiler.ts"),
        "import { runtime } from './runtime.ts'; export const generated = () => \"import { runtime } from 'runtime/runtime';\";",
    )
    .expect("write runtime-generated source");
    let target = root.path.join("src/runtime.ts");
    fs::write(&target, "export const runtime = 42;").expect("write runtime target");
    fs::write(
        &entry,
        "import { generated } from './src/compiler.ts'; console.log(generated());",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("Node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build graph");
    assert!(graph.runtime_package_aliases().iter().any(|alias| {
        alias.specifier == "runtime/runtime" && alias.target == fs::canonicalize(&target).unwrap()
    }));
    let plan = native_graph_plan(&graph).expect("plan graph");
    let canonical_target = fs::canonicalize(target).unwrap();
    assert!(
        !plan.excluded.contains(&canonical_target),
        "resolver-approved runtime aliases need their target linked"
    );
    assert!(plan.runtime_alias_live.contains(&canonical_target));
    let mut demand = kiln_liveness::executable_demand(
        &graph,
        &selection_options_with_exclusions(&NativeProfile::Dev, &plan.excluded),
    )
    .expect("select static entry");
    assert!(!demand.live.contains(&canonical_target));
    include_runtime_aliases_in_demand(&graph, &mut demand, &plan.runtime_alias_live);
    assert!(demand.live.contains(&canonical_target));
    assert_eq!(demand.slots[&canonical_target].len(), 1);
    let (reduced, _) = graph.reduce_to_live(&demand).unwrap();
    assert!(
        reduced.runtime_package_aliases().iter().any(|alias| {
            alias.specifier == "runtime/runtime" && alias.target == canonical_target
        })
    );
}

#[test]
fn native_runtime_failures_keep_distinct_captured_bases() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-runtime-failure-bases");
    let base_a = root.path.join("a.cjs");
    let base_b = root.path.join("b.cjs");
    fs::write(&base_a, "module.exports = 1;").expect("write base a");
    fs::write(&base_b, "module.exports = 1;").expect("write base b");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        format!(
            "import {{ createRequire }} from 'node:module'; createRequire('{}')('node:zlib'); createRequire('{}')('node:zlib');\n",
            base_a.display(),
            base_b.display()
        ),
    )
    .expect("write runtime failure entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build runtime failure graph");
    let reject_zlib = |graph: &ModuleGraph, node: &kiln_module_graph::ModuleNode| {
        if node.source == ModuleSource::NodeBuiltin(kiln_module_resolver::NodeBuiltin::Zlib) {
            return Err(NativeError::UnsupportedNodeBuiltin {
                specifier: "node:zlib",
            });
        }
        validate_native_builtin_node(graph, node)
    };
    let plan = native_graph_plan_with_validator(&graph, &reject_zlib)
        .expect("policy-rejected runtime failures are deferred");
    assert_eq!(plan.runtime_failures.len(), 2);
}

#[test]
fn native_dynamic_and_runtime_same_specifier_have_separate_frontiers() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-lazy-frontier-kind");
    let package = root.path.join("pkg");
    fs::create_dir_all(&package).expect("create package");
    fs::write(
        package.join("package.json"),
        r#"{"exports":{"require":"./c.cjs","import":"./i.mjs"}}"#,
    )
    .expect("write package exports");
    fs::write(package.join("c.cjs"), "module.exports = 1;").expect("write cjs target");
    fs::write(package.join("i.mjs"), "export default 2;").expect("write esm target");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "import { createRequire } from 'node:module'; createRequire(import.meta.url)('./pkg'); import('./pkg');\n",
    )
    .expect("write shared lazy frontier entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build shared lazy frontier graph");
    let plan = native_graph_plan(&graph).expect("both lazy frontiers are supported");
    assert!(plan.failures.is_empty());
    assert!(plan.runtime_failures.is_empty());
    assert_eq!(plan.excluded.len(), 0);
}

#[test]
fn native_failed_dynamic_closure_preserves_shared_successful_dependencies() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping dynamic closure sharing integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-dynamic-closure-sharing");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    fs::write(
        root.path.join("shared.mjs"),
        "globalThis.sharedCount = (globalThis.sharedCount || 0) + 1; export const count = globalThis.sharedCount;\n",
    )
    .expect("write shared dependency");
    fs::write(
        root.path.join("bad.mjs"),
        "import './shared.mjs'; import './missing-static.mjs'; export const value = 1;\n",
    )
    .expect("write failed dynamic target");
    fs::write(
        root.path.join("good.mjs"),
        "import './shared.mjs'; export const value = 42;\n",
    )
    .expect("write successful dynamic target");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "import { count } from './shared.mjs';\n\
         try { await import('./bad.mjs'); } catch (error) { console.log(error instanceof TypeError); }\n\
         const good = await import('./good.mjs');\n\
         console.log(good.value, count, globalThis.sharedCount);\n",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build sharing graph");
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("run sharing graph");
    assert_eq!(
        report.exit_code,
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&report.stderr)
    );
    assert_eq!(report.stdout, b"true\n42 1 1\n");
}

#[test]
fn native_dynamic_top_level_await_import_waits_for_completion() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping dynamic TLA integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-dynamic-tla");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    fs::write(
        root.path.join("dep.mjs"),
        "globalThis.dynamicTlaCount = (globalThis.dynamicTlaCount || 0) + 1;\nexport const x = await new Promise(resolve => setTimeout(() => resolve(42), 1));\n",
    )
    .expect("write dependency");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "const a = import('./dep.mjs'); const b = import('./dep.mjs'); const [na, nb] = await Promise.all([a, b]); console.log(na.x, nb.x, na === nb, globalThis.dynamicTlaCount);\n",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build dynamic TLA graph");
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("dynamic TLA graph builds and runs");
    assert_eq!(report.exit_code, Some(0), "stderr={:?}", report.stderr);
    assert_eq!(report.stdout, b"42 42 true 1\n");
}

#[test]
fn native_dynamic_tla_rejection_preserves_original_error() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Some(toolchain) = Toolchain::discover().ok() else {
        return;
    };
    let root = TestDirectory::new("kiln-native-dynamic-tla-rejection");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).unwrap();
    fs::write(root.path.join("sentinel.mjs"), "globalThis.dynamicTlaRejectCount = (globalThis.dynamicTlaRejectCount || 0) + 1;\nawait Promise.resolve();\nthrow globalThis.sentinel;\n").unwrap();
    fs::write(root.path.join("undefined.mjs"), "globalThis.dynamicTlaUndefinedCount = (globalThis.dynamicTlaUndefinedCount || 0) + 1;\nawait Promise.resolve();\nthrow undefined;\n").unwrap();
    let entry = root.path.join("main.mjs");
    fs::write(&entry, "const sentinel = { marker: 42 }; globalThis.sentinel = sentinel;\nconst a = import('./sentinel.mjs').then(() => null, error => error);\nconst b = import('./sentinel.mjs').then(() => null, error => error);\nconst u1 = import('./undefined.mjs').then(() => null, error => error);\nconst u2 = import('./undefined.mjs').then(() => null, error => error);\nconst [ea, eb, eu1, eu2] = await Promise.all([a, b, u1, u2]);\nconsole.log(ea === sentinel, eb === sentinel, ea === eb, globalThis.dynamicTlaRejectCount, eu1 === undefined, eu2 === undefined, globalThis.dynamicTlaUndefinedCount);\n").unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .unwrap();
    assert_eq!(report.exit_code, Some(0), "stderr={:?}", report.stderr);
    assert_eq!(report.stdout, b"true true true 1 true true 1\n");
}

#[test]
fn native_static_top_level_await_resolves_before_entry() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping static top-level await integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-static-tla");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    fs::write(
        root.path.join("dep.mjs"),
        "export const x = await new Promise(resolve => setTimeout(() => resolve(42), 1));\n",
    )
    .expect("write dependency");
    let entry = root.path.join("main.mjs");
    fs::write(&entry, "import { x } from './dep.mjs'; console.log(x);\n").expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build static top-level await graph");
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("static top-level await graph builds and runs");
    assert_eq!(
        report.exit_code,
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&report.stderr)
    );
    assert_eq!(
        report.stdout,
        b"42\n",
        "exit={:?} stderr={}",
        report.exit_code,
        String::from_utf8_lossy(&report.stderr)
    );
}

#[test]
fn a_second_build_reuses_every_material_artifact_and_runs() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native integration: no C compiler found");
        return;
    };
    let root = std::env::temp_dir().join(format!(
        "kiln-native-test-{}-{}",
        std::process::id(),
        WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let cache = root.join("cache");
    let first_output = root.join("first");
    let second_output = root.join("second");
    let builder = NativeBuilder::new(ArtifactCache::new(cache), toolchain);
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        constants: vec![
            Constant::String("native".into()),
            Constant::Number(42.0_f64.to_bits()),
        ],
        code: vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 2,
            },
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };

    let first = builder
        .build(
            &program,
            &NativeBuildOptions {
                output: first_output,
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("first native build succeeds");
    assert!(!first.cache_hits.application_object);
    assert!(!first.cache_hits.executable);

    let second = builder
        .run(
            &program,
            &NativeBuildOptions {
                output: second_output,
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("cached native build and run succeeds");
    assert!(second.build.cache_hits.generated_c);
    assert!(second.build.cache_hits.application_object);
    assert!(second.build.cache_hits.runtime_object);
    assert!(second.build.cache_hits.launcher_object);
    assert!(second.build.cache_hits.executable);
    assert_eq!(second.build.timings.compile, Duration::ZERO);
    assert_eq!(second.build.timings.link, Duration::ZERO);
    assert_eq!(second.exit_code, Some(0));
    assert_eq!(second.stdout, b"native 42\n");
    assert!(second.stderr.is_empty());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn owned_result_scalar_api_copies_strings_and_is_idempotent() {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("skipping owned-result native integration on unsupported platform");
        return;
    }
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping owned-result native integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-owned-result-test");
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain.clone());
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        constants: vec![Constant::String("left\0right".into())],
        code: vec![Instruction::Const(0), Instruction::Return],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let report = builder
        .build_shared_library(
            &program,
            &NativeBuildOptions {
                output: root.path.join(shared_library_name()),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("build owned-result scalar library");
    assert_eq!(
        report.module_api_symbol,
        format!("{}_api_v2", report.module_symbol)
    );
    let header_name = report
        .header
        .file_name()
        .and_then(|name| name.to_str())
        .expect("owned-result header file name");
    let host_source = root.path.join("host.c");
    let host_object = root.path.join("host.o");
    let host_executable = root.path.join("host");
    fs::write(
        &host_source,
        format!(
            r#"#include "{header_name}"
#include <string.h>

int main(void) {{
    const kiln_module_api_v2 *api = {api_symbol}();
    kiln_result_v2 *owned = NULL;
    kiln_value value;
    kiln_value legacy;
    int index;
    if (api == NULL || api->abi_version != KILN_MODULE_API_VERSION ||
        api->struct_size < sizeof(*api) || api->run == NULL || api->read == NULL ||
        api->dispose == NULL) return 1;
    if ({module_symbol}(NULL, &legacy) != KILN_C99_OK) return 21;
    if (legacy.tag != KILN_VALUE_STRING) return 22;
    if (legacy.flags != 0) return 23;
    if (legacy.len != 10) return 24;
    if (!kiln_string_equals_ascii((const kiln_string *)legacy.ptr, "left\0right", 10)) return 25;
    for (index = 0; index < 64; ++index) {{
        if (api->run(NULL, &owned) != KILN_C99_OK || owned == NULL) return 3;
        if (api->read(owned, &value) != KILN_C99_OK || value.tag != KILN_VALUE_STRING ||
            value.flags != 0 ||
            value.len != 10 || value.ptr == NULL || !kiln_string_equals_ascii((const kiln_string *)value.ptr, "left\0right", 10))
            return 4;
        api->dispose(&owned);
        if (owned != NULL) return 5;
        api->dispose(&owned);
    }}
    return 0;
}}
"#,
            api_symbol = report.module_api_symbol,
            module_symbol = report.module_symbol,
        ),
    )
    .expect("write owned-result host");
    let mut compile_options = CompileOptions::default();
    compile_options.include_dirs.push(root.path.clone());
    toolchain
        .compile_c(&host_source, &host_object, &compile_options)
        .expect("compile owned-result host");
    let mut link_options = LinkOptions::default();
    link_options.runtime_libraries.push(report.output.clone());
    let loader_directory = report.loader.parent().expect("loader has directory");
    link_options
        .extra_args
        .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
    toolchain
        .link_executable(&[host_object], &host_executable, &link_options)
        .expect("link owned-result host");
    let mut command = Command::new(&host_executable);
    #[cfg(target_os = "linux")]
    command.env("LD_LIBRARY_PATH", loader_directory);
    #[cfg(target_os = "macos")]
    command.env("DYLD_LIBRARY_PATH", loader_directory);
    let output = command
        .output_when_launchable()
        .expect("run owned-result host");
    assert!(
        output.status.success(),
        "owned-result host failed with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    #[cfg(target_os = "linux")]
    {
        let symbols = Command::new("nm")
            .args(["-D", "--defined-only"])
            .arg(&report.loader)
            .output()
            .expect("nm is available");
        assert!(symbols.status.success());
        let mut exported = String::from_utf8_lossy(&symbols.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().last())
            .filter(|symbol| symbol.starts_with("kiln_"))
            .map(|symbol| symbol.split('@').next().unwrap_or(symbol).to_owned())
            .collect::<Vec<_>>();
        exported.sort_unstable();
        let mut expected = vec![report.module_symbol, report.module_api_symbol];
        expected.sort_unstable();
        assert_eq!(exported, expected);
    }
}

#[test]
fn owned_result_scalar_api_rejects_malformed_host_strings() {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!(
            "skipping owned-result scalar malformed-host integration on unsupported platform"
        );
        return;
    }
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping owned-result scalar malformed-host integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-owned-result-malformed-scalar-test");
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        constants: Vec::new(),
        code: vec![
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 0,
            },
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain.clone())
        .build_shared_library(
            &program,
            &NativeBuildOptions {
                output: root.path.join(shared_library_name()),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("build malformed-host scalar library");
    let header_name = report
        .header
        .file_name()
        .and_then(|name| name.to_str())
        .expect("malformed-host scalar header file name");
    let host_source = root.path.join("host.c");
    fs::write(
        &host_source,
        format!(
            r#"#include "{header_name}"

static int malformed_call(void *context, uint16_t operation, uint16_t argc,
                          const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    (void)operation;
    (void)argc;
    (void)arguments;
    if (result == NULL) return KILN_C99_HOST_ERROR;
    *result = (kiln_value){{ .tag = KILN_VALUE_STRING, .flags = 0, .len = 3, .bits = 0 }};
    return KILN_C99_OK;
}}

int main(void) {{
    kiln_host host = {{malformed_call, NULL, NULL}};
    const kiln_module_api_v2 *api = {api_symbol}();
    kiln_result_v2 *owned = NULL;
    if (api == NULL) return 1;
    if (api->run(&host, &owned) != KILN_C99_INVALID_ARGUMENT || owned != NULL)
        return 2;
    api->dispose(&owned);
    return 0;
}}
"#,
            api_symbol = report.module_api_symbol,
        ),
    )
    .expect("write malformed-host scalar host");
    let host_object = root.path.join("host.o");
    let mut compile_options = CompileOptions::default();
    compile_options.include_dirs.push(root.path.clone());
    toolchain
        .compile_c(&host_source, &host_object, &compile_options)
        .expect("compile malformed-host scalar host");
    let host_executable = root.path.join("host");
    let mut link_options = LinkOptions::default();
    link_options.runtime_libraries.push(report.output.clone());
    let loader_directory = report.loader.parent().expect("loader has directory");
    link_options
        .extra_args
        .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
    toolchain
        .link_executable(&[host_object], &host_executable, &link_options)
        .expect("link malformed-host scalar host");
    let mut command = Command::new(&host_executable);
    #[cfg(target_os = "linux")]
    command.env("LD_LIBRARY_PATH", loader_directory);
    #[cfg(target_os = "macos")]
    command.env("DYLD_LIBRARY_PATH", loader_directory);
    let output = command
        .output_when_launchable()
        .expect("run malformed-host scalar host");
    assert!(
        output.status.success(),
        "malformed-host scalar failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn concurrent_runs_with_one_public_output_launch_their_private_program() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native concurrency integration: no C compiler found");
        return;
    };
    let root = std::env::temp_dir().join(format!(
        "kiln-native-race-test-{}-{}",
        std::process::id(),
        WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let shared_output = root.join("shared-program");
    let builder = NativeBuilder::new(ArtifactCache::new(root.join("cache")), toolchain);
    let programs = [stdout_program("alpha", 1.0), stdout_program("beta", 2.0)];
    for program in &programs {
        builder
            .build(
                program,
                &NativeBuildOptions {
                    output: shared_output.clone(),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                    march_native: false,
                    ..NativeBuildOptions::default()
                },
            )
            .expect("prime each native executable");
    }

    let barrier = Arc::new(Barrier::new(12));
    let mut threads = Vec::new();
    for index in 0..12 {
        let builder = builder.clone();
        let barrier = Arc::clone(&barrier);
        let program = programs[index % programs.len()].clone();
        let output = shared_output.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            builder
                .run(
                    &program,
                    &NativeBuildOptions {
                        output,
                        profile: NativeProfile::Dev,
                        realm: NativeRealmPolicy::default(),
                        march_native: false,
                        ..NativeBuildOptions::default()
                    },
                )
                .expect("concurrent native run succeeds")
        }));
    }
    for (index, thread) in threads.into_iter().enumerate() {
        let expected: &[u8] = if index % 2 == 0 {
            b"alpha\n"
        } else {
            b"beta\n"
        };
        let report = thread.join().expect("run thread does not panic");
        assert_eq!(report.stdout, expected);
        let expected_result = if index % 2 == 0 { 1.0_f64 } else { 2.0_f64 };
        assert_eq!(
            report.result,
            Some(HostValue::Number(expected_result.to_bits()))
        );
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn native_and_interpreter_agree_across_the_scalar_control_flow_slice() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native equivalence integration: no C compiler found");
        return;
    };
    let root = std::env::temp_dir().join(format!(
        "kiln-native-equivalence-test-{}-{}",
        std::process::id(),
        WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let builder = NativeBuilder::new(ArtifactCache::new(root.join("cache")), toolchain);

    for (name, program) in scalar_equivalence_programs() {
        let mut host = kiln_runtime::CaptureHost::new();
        let interpreted = kiln_interpreter::execute(
            std::rc::Rc::new(program.clone()),
            &mut host,
            Default::default(),
            kiln_interpreter::ExecutionCapabilities::default(),
        )
        .unwrap_or_else(|error| panic!("interpreter fixture {name} failed: {error}"));
        let expected = host.into_stdout().into_bytes();
        let expected_result = HostValue::from(interpreted.value);
        let native = builder
            .run(
                &program,
                &NativeBuildOptions {
                    output: root.join(name),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                    march_native: false,
                    ..NativeBuildOptions::default()
                },
            )
            .unwrap_or_else(|error| panic!("native fixture {name} failed: {error}"));
        assert_eq!(native.exit_code, Some(0), "fixture {name}");
        assert_eq!(native.stdout, expected, "fixture {name}");
        assert!(native.stderr.is_empty(), "fixture {name}");
        assert_eq!(native.result, Some(expected_result), "fixture {name}");
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn native_json_errors_are_catchable_and_uncaught_errors_are_reported() {
    use kiln_module_resolver::ModuleResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native JSON exception integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-json-exceptions");
    let project = root.path.join("project");
    fs::create_dir_all(&project).expect("create JSON exception project");
    let entry = project.join("main.js");
    fs::write(
            &entry,
            "try { JSON.parse('{oops'); } catch (error) { console.log('parse', error instanceof SyntaxError, error.constructor.name); }\nconsole.log('after-parse');\nconst cycle = {}; cycle.self = cycle; try { JSON.stringify(cycle); } catch (error) { console.log('stringify', error instanceof TypeError, error.constructor.name); }\nconsole.log('after-stringify');\n",
        )
        .expect("write catchable JSON exception source");
    let mut graph = ModuleGraph::build_node(
        &entry,
        ModuleResolver::new(&project).expect("module resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("catchable JSON exception source compiles");
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let caught = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("caught"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("catchable JSON exceptions run natively");
    assert_eq!(caught.exit_code, Some(0));
    assert_eq!(
        caught.stdout,
        b"parse true SyntaxError\nafter-parse\nstringify true TypeError\nafter-stringify\n"
    );
    assert!(caught.stderr.is_empty());

    fs::write(&entry, "JSON.parse('{oops');\n").expect("write uncaught JSON exception source");
    graph = ModuleGraph::build_node(
        &entry,
        ModuleResolver::new(&project).expect("module resolver for uncaught source"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("uncaught JSON exception source compiles");
    let uncaught = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("uncaught"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("uncaught JSON exception run reports a native result");
    assert_eq!(uncaught.exit_code, Some(1));
    assert!(uncaught.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&uncaught.stderr)
            .contains("uncaught SyntaxError: Unexpected token in JSON")
    );

    fs::write(
        &entry,
        "const cycle = {}; cycle.self = cycle; JSON.stringify(cycle);\n",
    )
    .expect("write uncaught cyclic JSON source");
    graph = ModuleGraph::build_node(
        &entry,
        ModuleResolver::new(&project).expect("module resolver for cyclic source"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("uncaught cyclic JSON source compiles");
    let uncaught_cycle = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("uncaught-cycle"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("uncaught cyclic JSON run reports a native result");
    assert_eq!(uncaught_cycle.exit_code, Some(1));
    assert!(uncaught_cycle.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&uncaught_cycle.stderr)
            .contains("uncaught TypeError: Converting circular structure to JSON")
    );
}

#[test]
fn native_host_clock_is_explicit_while_default_date_stays_at_epoch() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native host-clock integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-host-clock");
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        constants: vec![Constant::String("now".into())],
        code: vec![
            Instruction::LoadBuiltin(kiln_ir::Builtin::Date),
            Instruction::CallMethod { name: 0, argc: 0 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };

    let deterministic = builder
        .run(
            &program,
            &NativeBuildOptions {
                output: root.path.join("deterministic"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("deterministic Date program runs");
    assert_eq!(deterministic.result, Some(HostValue::number(0.0)));

    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock is after epoch")
        .as_secs_f64()
        * 1_000.0;
    let host_backed = builder
        .run(
            &program,
            &NativeBuildOptions {
                output: root.path.join("host"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::host_clock(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("host-backed Date program runs");
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock is after epoch")
        .as_secs_f64()
        * 1_000.0;
    let observed = host_backed
        .result
        .as_ref()
        .and_then(HostValue::as_number)
        .expect("Date.now returns a number");
    assert_eq!(
        observed,
        observed.trunc(),
        "Date.now must return integral milliseconds"
    );
    assert!(
        observed >= before && observed <= after,
        "host Date.now {observed} outside build/run interval {before}..={after}"
    );
    assert_ne!(
        deterministic.build.module_symbol,
        host_backed.build.module_symbol
    );
}

#[test]
fn native_host_process_completion_beats_realm_timeout() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native host process timing: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-host-process-timing");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "import { run } from 'kiln:process';\n\
         const result = await Promise.race([\n\
           run('/bin/sh', ['-c', 'sleep 0.35; printf done']).then(() => 'process'),\n\
           new Promise(resolve => setTimeout(() => resolve('timeout'), 1000)),\n\
         ]);\n\
         console.log(result);\n",
    )
    .expect("write process timing entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build host process timing graph");
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::host_clock(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("run host process timing graph");
    assert_eq!(report.stdout, b"process\n");
}

#[test]
fn native_entropy_policy_is_reproducible_or_explicitly_host_backed() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native entropy integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-entropy");
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        constants: vec![Constant::String("random".into())],
        code: vec![
            Instruction::LoadBuiltin(kiln_ir::Builtin::Math),
            Instruction::CallMethod { name: 0, argc: 0 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let run = |name: &str, entropy: NativeEntropyPolicy| {
        builder
            .run(
                &program,
                &NativeBuildOptions::new(root.path.join(name), NativeProfile::Dev)
                    .with_realm_policy(NativeRealmPolicy::default().with_entropy(entropy)),
            )
            .unwrap_or_else(|error| panic!("native entropy fixture {name} failed: {error}"))
    };

    let deterministic = run("deterministic", NativeEntropyPolicy::Deterministic);
    let seeded_a = run(
        "seeded-a",
        NativeEntropyPolicy::Seeded(kiln_runtime::DEFAULT_ENTROPY_SEED),
    );
    let seeded_b = run(
        "seeded-b",
        NativeEntropyPolicy::Seeded(kiln_runtime::DEFAULT_ENTROPY_SEED),
    );
    let other_seed = run("seeded-other", NativeEntropyPolicy::Seeded(7));
    assert_eq!(deterministic.result, seeded_a.result);
    assert_eq!(seeded_a.result, seeded_b.result);
    assert_eq!(seeded_a.build.module_symbol, seeded_b.build.module_symbol);
    assert_ne!(seeded_a.build.module_symbol, other_seed.build.module_symbol);
    assert_ne!(seeded_a.result, other_seed.result);

    let host_a = run("host-a", NativeEntropyPolicy::Host);
    let host_b = run("host-b", NativeEntropyPolicy::Host);
    for report in [&host_a, &host_b] {
        let value = report
            .result
            .as_ref()
            .and_then(HostValue::as_number)
            .expect("Math.random returns a number");
        assert!((0.0..1.0).contains(&value));
    }
    assert_eq!(host_a.build.module_symbol, host_b.build.module_symbol);
    assert_ne!(
        host_a.build.module_symbol,
        deterministic.build.module_symbol
    );
}

#[test]
fn native_deterministic_entropy_is_one_stream_across_modules() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-graph-entropy");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
        .expect("write package metadata");
    fs::write(
        root.path.join("first.mjs"),
        "export const first = Math.random();",
    )
    .expect("write first entropy consumer");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import { first } from "./first.mjs";
console.log(first === Math.random());"#,
    )
    .expect("write second entropy consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build entropy graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native graph entropy integration: no C compiler found");
        return;
    };
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    for (name, entropy) in [
        ("default", NativeEntropyPolicy::Deterministic),
        ("seeded", NativeEntropyPolicy::Seeded(7)),
    ] {
        let report = builder
            .run_graph(
                &graph,
                &NativeBuildOptions::new(root.path.join(name), NativeProfile::Dev)
                    .with_realm_policy(NativeRealmPolicy::default().with_entropy(entropy)),
            )
            .unwrap_or_else(|error| panic!("native graph entropy fixture failed: {error}"));
        assert_eq!(
            report.exit_code,
            Some(0),
            "stderr={}",
            String::from_utf8_lossy(&report.stderr)
        );
        assert_eq!(report.stdout, b"false\n");
    }
}

#[test]
fn native_await_promise_resolve_preserves_generic_and_ordering_semantics() {
    let Some(output) = run_native_release_fixture(
        "native-await-promise-resolve-semantics",
        r#"
const events = [];
async function primitive() {
  events.push("before");
  const value = await Promise.resolve(1);
  events.push(`after:${value}`);
}

const thenable = { then(resolve) { events.push("thenable"); resolve(2); } };
const existing = Promise.resolve(3);
const originalResolve = Promise.resolve;
const subclass = class extends Promise {};
primitive();
events.push("outside");
Promise.resolve(thenable).then(value => events.push(`then:${value}`));
Promise.resolve(existing).then(value => events.push(`existing:${value}`));
Promise.resolve.call(subclass, 4).then(value => events.push(`sub:${value}`));
Promise.resolve = function (value) { events.push("override"); return originalResolve.call(this, value); };
Promise.resolve(5).then(value => events.push(`override:${value}`));
setTimeout(() => console.log(events.join("|")), 0);
"#,
    ) else {
        return;
    };
    assert_eq!(
        output,
        "before|outside|override|after:1|thenable|existing:3|sub:4|override:5|then:2\n"
    );
}

#[test]
fn native_await_promise_resolve_emits_guard_for_microtasks_shape() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("native-await-promise-resolve-emission");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"
export async function microtasks() {
  let counter = 0;
  for (let i = 0; i < 20000; i++) {
    await Promise.resolve(i);
    counter += 1;
  }
  return counter;
}
"#,
    )
    .expect("write microtasks emission fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build microtasks emission graph");
    let emitted = emit_native_graph_module_with_policy(
        graph.module(graph.entry()).expect("entry module"),
        "main.mjs",
        &kiln_backend_c::EmissionPolicy::default(),
    )
    .expect("emit microtasks fixture");
    let guard_start = emitted
        .source
        .find("kiln_c99_builtin_member_unmodified(KILN_C99_BUILTIN_PROMISE_PROTOTYPE, \"constructor\") && KILN_VAL_IS_BUILTIN")
        .expect("Promise.resolve await guard emitted");
    let handled_end = emitted.source[guard_start..]
        .find("handled = 1;")
        .map(|offset| guard_start + offset + "handled = 1;".len())
        .expect("guarded await path handles its result");
    let guard = &emitted.source[guard_start..handled_end];
    assert!(guard.contains("KILN_VAL_IS_NUMBER(v"), "{guard}");
    assert!(
        guard.contains("KILN_C99_METHOD_PROMISE_RESOLVE"),
        "prepared await fusion checks the saved callee's identity: {guard}"
    );
    assert!(guard.contains("handled = 1;"), "{guard}");
}

/// Realms issue 20: a typed method entry that inlines `Math.sqrt` is entered only after its call
/// site asks about that one member. Reading the mutable `Math` global can itself invoke an
/// accessor, so the entry checks the member again after that read and keeps its boxed fallback.
#[test]
fn typed_entry_call_sites_ask_about_the_members_the_entry_inlines() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("typed-entry-relies-emission");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"
class Point { constructor(x, y) { this.x = x; this.y = y; }
              dist() { return Math.sqrt(this.x * this.x + this.y * this.y); } }
const pts = []; for (let i = 0; i < 1000; i++) pts.push(new Point(i, i + 1));
let s = 0; for (const p of pts) s += p.dist();
console.log(s);
"#,
    )
    .expect("write typed entry emission fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build typed entry emission graph");
    let emitted = emit_native_graph_module_with_policy(
        graph.module(graph.entry()).expect("entry module"),
        "main.mjs",
        &kiln_backend_c::EmissionPolicy::default(),
    )
    .expect("emit typed entry fixture");
    let source = &emitted.source;
    let sqrt = "kiln_c99_builtin_member_unmodified(KILN_C99_BUILTIN_MATH, \"sqrt\")";
    assert!(
        source.contains(&format!("kiln_typed_ok && ({sqrt})")),
        "the typed arm asks about Math.sqrt before entering"
    );
    let definition = source
        .lines()
        .find(|line| line.contains("_t(int depth") && line.trim_end().ends_with('{'))
        .expect("typed entry defined");
    let body = &source[source.find(definition).expect("definition in source")..];
    let body = &body[..body.find("\n}\n").expect("typed entry ends")];
    assert!(
        body.contains(&format!("if (KILN_LIKELY({sqrt}))")),
        "the entry checks after its observable global read: {body}"
    );
    assert!(
        !body.contains("kiln_c99_builtins_unmodified()"),
        "the entry asks about no other built-in: {body}"
    );
}

#[test]
fn native_await_promise_resolve_honors_constructor_assignment() {
    let Some(output) = run_native_release_fixture(
        "native-await-promise-resolve-constructor-assignment",
        r#"
Promise.prototype.constructor = function AssignedPromise() {};
async function run() {
  const events = [];
  events.push("before");
  const value = await Promise.resolve(7);
  events.push(`after:${value}`);
  setTimeout(() => console.log(events.join("|")), 0);
}
run();
console.log("outside");
"#,
    ) else {
        return;
    };
    assert_eq!(output, "outside\nbefore|after:7\n");
}

#[test]
fn native_await_promise_resolve_falls_back_for_non_number() {
    let Some(output) = run_native_release_fixture(
        "native-await-promise-resolve-non-number",
        r#"
async function run() {
  console.log(await Promise.resolve("text"));
}
run();
"#,
    ) else {
        return;
    };
    assert_eq!(output, "text\n");
}

#[test]
fn native_await_promise_resolve_falls_back_after_override() {
    let Some(output) = run_native_release_fixture(
        "native-await-promise-resolve-await-override",
        r#"
const originalResolve = Promise.resolve;
Promise.resolve = function (value) { return originalResolve.call(this, value + 1); };
async function run() {
  console.log(await Promise.resolve(6));
}
run();
"#,
    ) else {
        return;
    };
    assert_eq!(output, "7\n");
}

#[test]
fn native_prepared_calls_keep_the_callee_read_before_argument_effects() {
    let Some(output) = run_native_release_fixture(
        "native-prepared-call-callee-before-arguments",
        r#"
let replacementResolveCalls = 0;
let resolveArgumentCalls = 0;
function replaceResolve() {
  resolveArgumentCalls++;
  Promise.resolve = function () { replacementResolveCalls += 1; return "replacement"; };
  return 41;
}
async function awaitValue() {
  return await Promise.resolve(replaceResolve());
}

const originalSqrt = Math.sqrt;
let replacementSqrtCalls = 0;
let sqrtArgumentCoercions = 0;
function replaceSqrtAndReturnNumber() {
  sqrtArgumentCoercions++;
  Math.sqrt = function () { replacementSqrtCalls += 1; return -1; };
  return 3;
}
class Point {
  constructor(value) { this.value = value; }
  distance() { return Math.sqrt(this.value); }
}
const points = [];
for (let i = 0; i < 1000; i++) points.push(new Point(i));
for (let i = 0; i < points.length; i++) points[i].distance();
// Coercion changes the public property after the method read. This call must still invoke the
// previously captured callee.
points[0].value = { valueOf: replaceSqrtAndReturnNumber };

Promise.resolve(awaitValue()).then(value => {
  const distance = points[0].distance();
  console.log(`${value}|${distance}|${resolveArgumentCalls}|${replacementResolveCalls}|${sqrtArgumentCoercions}|${replacementSqrtCalls}|${Math.sqrt === originalSqrt}`);
});
"#,
    ) else {
        return;
    };
    assert_eq!(output, "41|1.7320508075688772|1|0|1|0|false\n");
}

#[test]
fn native_await_promise_resolve_honors_prototype_define_property() {
    let Some(output) = run_native_release_fixture(
        "native-await-promise-resolve-prototype-define-property",
        r#"
Object.defineProperty(Promise.prototype, "constructor", {
  get() { return Promise; }
});
async function run() {
  const events = [];
  events.push("before");
  const value = await Promise.resolve(7);
  events.push(`after:${value}`);
  setTimeout(() => console.log(events.join("|")), 0);
}
run();
console.log("outside");
"#,
    ) else {
        return;
    };
    assert_eq!(output, "outside\nbefore|after:7\n");
}

#[test]
fn release_source_dce_keeps_a_discarded_coercion_and_a_throwing_global_call() {
    // Issue 19: Oxc's dead-code pass assumes coercions are pure and `decodeURIComponent` never
    // throws; the release build deleted both statements and folded `calls` to 0.
    let Some(output) = run_native_release_fixture(
        "release-source-dce-observable-effects",
        r#"
let calls = 0;
const o = { valueOf() { calls += 1; return 2; }, toString() { calls += 10; return "o"; } };
o < 10;
o == 2;
`${o}`;
let thrown = "no";
try { decodeURIComponent("%"); } catch (error) { thrown = error.name; }
let hint = "none";
const d = new Date({ [Symbol.toPrimitive](h) { hint = h; return 0; } });
console.log(calls, thrown, hint, d.getTime());
"#,
    ) else {
        return;
    };
    assert_eq!(output, "12 URIError default 0\n");
}

#[test]
fn release_integer_remainder_of_a_negative_dividend_is_negative_zero() {
    // Issue 20: `(value | 0) % 2` was proven an int, and its `int64_t` twin lost the sign.
    let Some(output) = run_native_release_fixture(
        "release-integer-remainder-negative-zero",
        r#"
function constantRemainder(value) { return (value | 0) % 2; }
console.log([-4, -6, 4].map(constantRemainder).map((x) => Object.is(x, -0)).join());
function viaArgument(value) { return 1 / ((value | 0) % 5); }
console.log(viaArgument(-10), viaArgument(10));
"#,
    ) else {
        return;
    };
    assert_eq!(output, "true,true,false\n-Infinity Infinity\n");
}

#[test]
fn release_typed_method_call_takes_the_boxed_call_for_a_receiver_without_a_row() {
    // Issue 21: a receiver of the right class whose `v` is a string has no double row, so the
    // typed entry could only refuse; the call site now asks first and takes the boxed call.
    let Some(output) = run_native_release_fixture(
        "release-typed-method-receiver-without-row",
        r#"
class V { constructor(v) { this.v = v; } val() { return this.v; } }
const objs = [new V(1), new V("x"), new V(3)];
function sumVals() { let s = 0; for (let i = 0; i < objs.length; i++) s += objs[i].val(); return s; }
console.log(sumVals());
"#,
    ) else {
        return;
    };
    assert_eq!(output, "1x3\n");
}
