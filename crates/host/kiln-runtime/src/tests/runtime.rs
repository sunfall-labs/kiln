use crate::*;

#[test]
fn two_lanes_run_local_tasks_on_one_wall_reactor() {
    let reactor = Reactor::shared_wall().expect("shared reactor builds");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let lanes = (0..2)
        .map(|_| {
            let reactor = std::sync::Arc::clone(&reactor);
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                let executor = Executor::on_reactor(reactor);
                let lane = std::thread::current().id();
                barrier.wait();
                let local = executor.block_on(async {
                    let (send, receive) = tokio::sync::oneshot::channel();
                    tokio::task::spawn_local(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                        send.send(std::thread::current().id())
                            .expect("local task replies");
                    });
                    receive.await.expect("local task completes")
                });
                assert_eq!(local, lane, "local task stayed on its scheduler lane");
                lane
            })
        })
        .collect::<Vec<_>>();
    let first = lanes
        .into_iter()
        .map(|lane| lane.join().expect("lane exits"))
        .collect::<Vec<_>>();
    assert_ne!(first[0], first[1], "two lanes used different OS threads");
}

#[test]
fn local_task_wakes_a_scheduler_lane_without_parking_it() {
    let executor = Executor::on_reactor(Reactor::shared_wall().expect("reactor"));
    let queue = crate::turn_queue::ReadyQueue::new();
    let ticket = queue.ticket(42);
    let waker = std::task::Waker::from(std::sync::Arc::clone(&ticket));
    let lane = std::thread::current().id();
    let (signal, receive) = tokio::sync::oneshot::channel::<()>();
    let observed = std::rc::Rc::new(std::cell::Cell::new(None));
    executor.scope(|| {
        let observed = std::rc::Rc::clone(&observed);
        tokio::task::spawn_local(async move {
            receive.await.expect("signal arrives");
            observed.set(Some(std::thread::current().id()));
        });
    });

    ticket.wake();
    let lease = queue.take().expect("first turn");
    executor.poll_local_once(&waker);
    assert_eq!(observed.get(), None);
    lease.finish(crate::turn_queue::TurnOutcome::Parked);
    assert!(
        queue
            .take_timeout(std::time::Duration::from_millis(1))
            .is_none()
    );
    signal.send(()).expect("receiver still alive");
    let lease = queue
        .take_timeout(std::time::Duration::from_millis(100))
        .expect("the local task wakes the scheduler ticket");
    assert_eq!(lease.realm_id(), 42);
    executor.poll_local_once(&waker);
    assert_eq!(observed.get(), Some(lane));
    lease.finish(crate::turn_queue::TurnOutcome::Done);
}

#[test]
fn a_virtual_lane_advances_a_timer_after_a_local_task_yields() {
    let executor = Executor::new().expect("current-thread reactor");
    executor.pause_time();
    let queue = crate::turn_queue::ReadyQueue::new();
    let ticket = queue.ticket(43);
    let waker = std::task::Waker::from(ticket);
    let fired = std::rc::Rc::new(std::cell::Cell::new(false));
    executor.scope(|| {
        let fired = std::rc::Rc::clone(&fired);
        tokio::task::spawn_local(async move {
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            fired.set(true);
        });
    });
    for _ in 0..3 {
        executor.poll_local_once(&waker);
    }
    // Tokio's next timer tick can lie just beyond the requested millisecond.
    executor.advance_virtual_time(std::time::Duration::from_millis(2));
    for _ in 0..3 {
        executor.poll_local_once(&waker);
    }
    assert!(fired.get(), "the paused reactor must deliver the timer");
}

#[cfg(unix)]
#[test]
fn tokio_host_runs_process_with_bidirectional_pipes_and_preserves_status() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    let request = runtime.start_host_op(
        AsyncHostOp::ProcessRun {
            command: "sh".into(),
            args: vec![
                "-c".into(),
                "head -c 131072 /dev/zero & cat >/dev/null; wait; printf stderr >&2".into(),
            ],
            stdin: Some(vec![b'x'; 131072]),
        },
        false,
    );
    let completion = next_host_completion(&mut runtime).expect("process completes");
    assert_eq!(completion.request, request);
    let AsyncHostValue::ProcessOutput(output) = completion.result.expect("process succeeds") else {
        panic!("expected process output")
    };
    assert_eq!(output.stdout.len(), 131072);
    assert_eq!(output.stderr, b"stderr");
    assert_eq!(output.exit_code, Some(0));
    assert_eq!(output.signal, None);
}

#[cfg(unix)]
#[test]
fn tokio_host_process_result_preserves_signal_termination() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::ProcessRun {
            command: "sh".into(),
            args: vec!["-c".into(), "kill -TERM $$".into()],
            stdin: None,
        },
        false,
    );
    let completion = next_host_completion(&mut runtime).expect("process completes");
    let AsyncHostValue::ProcessOutput(output) = completion.result.expect("process result") else {
        panic!("expected process output")
    };
    assert_eq!(output.exit_code, None);
    assert_eq!(output.signal, Some(libc::SIGTERM));
}

#[cfg(unix)]
#[test]
fn tokio_host_process_reports_nonzero_exit_and_missing_command() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::ProcessRun {
            command: "sh".into(),
            args: vec!["-c".into(), "printf failed >&2; exit 7".into()],
            stdin: None,
        },
        false,
    );
    let completion = next_host_completion(&mut runtime).expect("process completes");
    let AsyncHostValue::ProcessOutput(output) = completion.result.expect("process result") else {
        panic!("expected process output")
    };
    assert_eq!(output.exit_code, Some(7));
    assert_eq!(output.stderr, b"failed");

    runtime.start_host_op(
        AsyncHostOp::ProcessRun {
            command: "kiln-process-command-that-does-not-exist".into(),
            args: Vec::new(),
            stdin: None,
        },
        false,
    );
    let completion = next_host_completion(&mut runtime).expect("spawn failure completes");
    let error = completion.result.expect_err("missing command fails");
    assert_eq!(error.syscall, "spawn");
}

#[cfg(unix)]
#[test]
fn tokio_host_process_cancellation_kills_and_reaps_the_child() {
    use std::process::Command;
    use std::thread;

    let directory = TestDirectory::new("process-cancel");
    let marker = directory.join("pid");
    let marker_string = marker.to_string_lossy().into_owned();
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    let request = runtime.start_host_op(
        AsyncHostOp::ProcessRun {
            command: "sh".into(),
            args: vec![
                "-c".into(),
                "echo \"$$\" > \"$1\"; exec sleep 30".into(),
                "kiln-process-test".into(),
                marker_string.clone(),
            ],
            stdin: None,
        },
        true,
    );
    let executor = Rc::clone(runtime.executor());
    let mut pid = None;
    for _ in 0..100 {
        executor.block_on(yield_to_tasks());
        if let Ok(contents) = fs::read_to_string(&marker) {
            pid = Some(contents.trim().parse::<i32>().expect("child PID"));
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let pid = pid.expect("child published its PID");
    assert!(AsyncHostOps::cancel(runtime.host(), request));

    for _ in 0..100 {
        executor.block_on(yield_to_tasks());
        let status = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("ps is available");
        if status.stdout.iter().all(u8::is_ascii_whitespace) {
            return;
        }
        assert!(
            status.status.success(),
            "ps failed while inspecting live child: {:?}: {}",
            status.status,
            String::from_utf8_lossy(&status.stderr)
        );
        thread::sleep(Duration::from_millis(5));
    }
    panic!("cancelled child PID {pid} was not reaped");
}

#[cfg(unix)]
#[test]
fn tokio_host_shutdown_kills_and_reaps_a_pending_process() {
    use std::process::Command;
    use std::thread;

    let directory = TestDirectory::new("process-shutdown");
    let marker = directory.join("pid");
    let marker_string = marker.to_string_lossy().into_owned();
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::ProcessRun {
            command: "sh".into(),
            args: vec![
                "-c".into(),
                "echo \"$$\" > \"$1\"; exec sleep 30".into(),
                "kiln-process-test".into(),
                marker_string,
            ],
            stdin: None,
        },
        true,
    );
    let mut pid = None;
    for _ in 0..100 {
        Rc::clone(runtime.executor()).block_on(yield_to_tasks());
        if let Ok(contents) = fs::read_to_string(&marker) {
            pid = Some(contents.trim().parse::<i32>().expect("child PID"));
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let pid = pid.expect("child published its PID");
    assert!(runtime.shutdown());
    for _ in 0..100 {
        let status = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("ps is available");
        if status.stdout.iter().all(u8::is_ascii_whitespace) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("shutdown child PID {pid} was not reaped");
}

#[cfg(unix)]
#[test]
fn tokio_host_shutdown_drains_a_process_after_last_op_cancellation() {
    use std::process::Command;
    use std::thread;

    let directory = TestDirectory::new("process-cancel-shutdown");
    let marker = directory.join("pid");
    let marker_string = marker.to_string_lossy().into_owned();
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    let request = runtime.start_host_op(
        AsyncHostOp::ProcessRun {
            command: "sh".into(),
            args: vec![
                "-c".into(),
                "echo \"$$\" > \"$1\"; exec sleep 30".into(),
                "kiln-process-test".into(),
                marker_string,
            ],
            stdin: None,
        },
        true,
    );
    let mut pid = None;
    for _ in 0..100 {
        Rc::clone(runtime.executor()).block_on(yield_to_tasks());
        if let Ok(contents) = fs::read_to_string(&marker) {
            pid = Some(contents.trim().parse::<i32>().expect("child PID"));
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let pid = pid.expect("child published its PID");
    assert!(AsyncHostOps::cancel(runtime.host(), request));
    assert!(runtime.shutdown());
    for _ in 0..100 {
        let status = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("ps is available");
        if status.stdout.iter().all(u8::is_ascii_whitespace) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("cancelled shutdown child PID {pid} was not reaped");
}

/// A clock reading keeps the part of it that is shorter than a millisecond.
///
/// The native FFI once spelled this conversion `as_millis() as f64`, which truncates, while
/// the interpreter spelled it `as_secs_f64() * 1_000.0`. So `performance.now()` answered
/// whole milliseconds under one backend and sub-milliseconds under the other -- one call,
/// two precisions, chosen by which engine ran it. It made the native clock unable to measure
/// anything shorter than a millisecond, which is most of what a benchmark measures, and it
/// is the kind of two-engine divergence ADR 0043 exists to forbid.
#[test]
fn a_clock_reading_keeps_its_sub_millisecond_part() {
    let reading = crate::Duration::from_nanos(1_500_250);
    let milliseconds = crate::epoch_milliseconds(reading);
    assert!(
        (milliseconds - 1.500_25).abs() < 1e-9,
        "expected 1.50025 ms, got {milliseconds}"
    );
    assert!(
        milliseconds.fract() != 0.0,
        "resolution collapsed to whole milliseconds"
    );
}

fn next_host_completion<H: RealmHostProvider>(runtime: &mut RealmRuntime<H>) -> Option<Completion> {
    runtime
        .next_host_completion()
        .expect("realm runtime remains healthy")
}

fn request_events(events: &[RuntimeHttpEvent]) -> (u64, &node_http::HttpRequest, Vec<u8>) {
    let [
        RuntimeHttpEvent::RequestHead { id, request },
        middle @ ..,
        RuntimeHttpEvent::RequestEnd { id: end_id },
    ] = events
    else {
        panic!("expected request head/data/end events: {events:?}")
    };
    assert_eq!(id, end_id);
    let mut body = Vec::new();
    for event in middle {
        let RuntimeHttpEvent::RequestData { id: data_id, data } = event else {
            panic!("unexpected event between request head and end: {event:?}")
        };
        assert_eq!(id, data_id);
        body.extend_from_slice(data);
    }
    (*id, request, body)
}
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, AtomicOrdering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("kiln-runtime-{label}-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn join(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn entered_operations_run_under_the_owned_tokio_runtime() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    assert!(tokio::runtime::Handle::try_current().is_err());
    assert!(runtime.enter(|| tokio::runtime::Handle::try_current().is_ok()));
}

#[test]
fn active_realm_host_protocol_admits_persistent_resources() {
    fn submit_listener(host: &mut dyn ActiveAsyncHostOps) -> RequestId {
        host.start(
            AsyncHostOp::TcpListen {
                host: "127.0.0.1".into(),
                port: 0,
            },
            true,
        )
    }

    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    // This is the same trait object the interpreter calls from inside `RealmRuntime::enter`.
    // Unlike the old Node-only seam, the generic host protocol admits resource operations.
    let request = runtime
        .executor
        .local
        .block_on(&runtime.executor.reactor.runtime, async {
            submit_listener(&mut runtime.state.host)
        });
    let resource = match next_host_completion(&mut runtime)
        .expect("listen completes")
        .result
        .expect("listen succeeds")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener resource, got {other:?}"),
    };
    assert_eq!(resource.kind, ResourceKind::TcpListener);
    assert!(!ActiveAsyncHostOps::cancel(
        &mut runtime.state.host,
        request
    ));

    runtime.start_host_op(
        AsyncHostOp::CloseResource {
            resource: resource.id,
        },
        false,
    );
    next_host_completion(&mut runtime).expect("close completes");
}

#[test]
fn default_capabilities_preserve_the_deterministic_realm() {
    let runtime = RealmRuntime::new().expect("Tokio runtime builds");
    let capabilities = runtime.capabilities();
    assert_eq!(capabilities, &RealmCapabilities::default());
    assert_eq!(capabilities.entropy_seed(), Some(DEFAULT_ENTROPY_SEED));
    assert_eq!(
        capabilities.artifact_identity(),
        RealmCapabilityIdentity::default()
    );
}

#[test]
fn each_realm_runtime_owns_an_isolated_http_agent() {
    let origin = HttpOrigin::http("example.com", 80);
    let mut first = RealmRuntime::new().expect("runtime builds");
    let mut second = RealmRuntime::new().expect("runtime builds");

    assert_eq!(
        ActiveHttpRuntime::acquire_client_connection(&mut first, origin.clone(), 1),
        AgentDecision::Connect
    );
    assert!(ActiveHttpRuntime::client_connection_connected(
        &mut first,
        origin.clone(),
        ResourceId::new(41),
        1,
    ));
    assert_eq!(
        ActiveHttpRuntime::release_client_connection(&mut first, ResourceId::new(41), true),
        AgentRelease::Idle
    );
    assert_eq!(
        ActiveHttpRuntime::acquire_client_connection(&mut first, origin.clone(), 2),
        AgentDecision::Reuse(ResourceId::new(41))
    );
    assert_eq!(
        ActiveHttpRuntime::acquire_client_connection(&mut second, origin, 1),
        AgentDecision::Connect
    );
}

#[test]
fn capability_identity_is_unambiguous_and_excludes_host_values() {
    let host = RealmCapabilities {
        clock: ClockCapability::Host,
        entropy: EntropyCapability::Host,
        timezone: TimeZoneCapability::Host,
        locale: LocaleCapability::Host,
    };
    assert_eq!(
        host.artifact_identity(),
        RealmCapabilityIdentity {
            clock: CapabilityIdentity::Host,
            entropy: CapabilityIdentity::Host,
            timezone: CapabilityIdentity::Host,
            locale: CapabilityIdentity::Host,
        }
    );
    assert_eq!(host.entropy_seed(), None);

    let named = RealmCapabilities {
        entropy: EntropyCapability::Seeded(7),
        timezone: TimeZoneCapability::Named("UTC;locale=host".into()),
        locale: LocaleCapability::Named("en-US".into()),
        ..RealmCapabilities::default()
    };
    assert_eq!(
        named.artifact_identity(),
        RealmCapabilityIdentity {
            clock: CapabilityIdentity::Virtual,
            entropy: CapabilityIdentity::Seeded(7),
            timezone: CapabilityIdentity::Named("UTC;locale=host".into()),
            locale: CapabilityIdentity::Named("en-US".into()),
        }
    );
}

#[test]
fn tokio_host_reads_and_writes_owned_bytes() {
    let directory = TestDirectory::new("async-files");
    let path = directory.join("value.bin");
    let path = path.to_string_lossy().into_owned();
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    let write = runtime.start_host_op(
        AsyncHostOp::WriteFile {
            path: path.clone(),
            data: vec![0, 1, 2, 255],
        },
        true,
    );
    assert!(runtime.has_pending_host_work());
    assert!(runtime.host().has_referenced_work());
    assert_eq!(
        next_host_completion(&mut runtime)
            .expect("write completes")
            .request,
        write
    );
    assert!(!runtime.host().has_referenced_work());

    let read = runtime.start_host_op(AsyncHostOp::ReadFile { path }, true);
    let completion = next_host_completion(&mut runtime).expect("read completes");
    assert_eq!(completion.request, read);
    assert_eq!(
        completion.result,
        Ok(AsyncHostValue::Bytes(vec![0, 1, 2, 255]))
    );
    assert!(!runtime.has_pending_host_work());
}

#[test]
fn tokio_host_returns_portable_stat_metadata() {
    let directory = TestDirectory::new("async-stat");
    let file = directory.join("value.txt");
    fs::write(&file, b"kiln").expect("seed file");
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::Stat {
            path: file.to_string_lossy().into_owned(),
        },
        true,
    );
    assert_eq!(
        next_host_completion(&mut runtime)
            .expect("stat completes")
            .result,
        Ok(AsyncHostValue::FileStat(FileStat {
            size: 4,
            is_file: true,
            is_directory: false,
            is_symlink: false,
        }))
    );
}

#[test]
fn tokio_host_handles_large_file_reads_and_writes() {
    let directory = TestDirectory::new("async-large-files");
    let path = directory.join("large.bin");
    let path_str = path.to_string_lossy().into_owned();
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");

    // Write > 64 KiB (e.g. 100 KiB) to exercise thread pool write fallback
    let large_data = vec![42u8; 100 * 1024];
    let write = runtime.start_host_op(
        AsyncHostOp::WriteFile {
            path: path_str.clone(),
            data: large_data.clone(),
        },
        true,
    );
    assert_eq!(
        next_host_completion(&mut runtime)
            .expect("large write completes")
            .request,
        write
    );

    // Read > 128 KiB: write a 200 KiB file directly and read it via start_host_op
    let read_path = directory.join("read_large.bin");
    let read_path_str = read_path.to_string_lossy().into_owned();
    let read_data = vec![7u8; 200 * 1024];
    std::fs::write(&read_path, &read_data).expect("seed large file");

    let read = runtime.start_host_op(
        AsyncHostOp::ReadFile {
            path: read_path_str,
        },
        true,
    );
    let completion = next_host_completion(&mut runtime).expect("large read completes");
    assert_eq!(completion.request, read);
    assert_eq!(completion.result, Ok(AsyncHostValue::Bytes(read_data)));
}

#[test]
fn tokio_host_executes_extended_filesystem_operations() {
    let directory = TestDirectory::new("async-extended-fs");
    let dir_str = directory.0.to_string_lossy().into_owned();
    let file_a = directory.join("file_a.txt");
    let file_b = directory.join("file_b.txt");
    let file_c = directory.join("file_c.txt");
    let file_a_str = file_a.to_string_lossy().into_owned();
    let file_b_str = file_b.to_string_lossy().into_owned();
    let file_c_str = file_c.to_string_lossy().into_owned();

    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");

    // Write file_a
    runtime.start_host_op(
        AsyncHostOp::WriteFile {
            path: file_a_str.clone(),
            data: b"hello world".to_vec(),
        },
        true,
    );
    let _ = next_host_completion(&mut runtime).expect("write completes");

    // Copy file_a -> file_b
    runtime.start_host_op(
        AsyncHostOp::CopyFile {
            from: file_a_str.clone(),
            to: file_b_str.clone(),
        },
        true,
    );
    let res = next_host_completion(&mut runtime)
        .expect("copy completes")
        .result;
    assert_eq!(res, Ok(AsyncHostValue::Undefined));

    // Rename file_b -> file_c
    runtime.start_host_op(
        AsyncHostOp::Rename {
            from: file_b_str.clone(),
            to: file_c_str.clone(),
        },
        true,
    );
    let res = next_host_completion(&mut runtime)
        .expect("rename completes")
        .result;
    assert_eq!(res, Ok(AsyncHostValue::Undefined));

    // ReadDir
    runtime.start_host_op(
        AsyncHostOp::ReadDir {
            path: dir_str.clone(),
            with_types: true,
        },
        true,
    );
    let res = next_host_completion(&mut runtime)
        .expect("readdir completes")
        .result;
    match res {
        Ok(AsyncHostValue::DirEntries(entries)) => {
            let names: Vec<String> = entries.into_iter().map(|e| e.name).collect();
            assert!(names.contains(&"file_a.txt".to_string()));
            assert!(names.contains(&"file_c.txt".to_string()));
            assert!(!names.contains(&"file_b.txt".to_string()));
        }
        other => panic!("expected DirEntries, got {other:?}"),
    }

    // Lstat file_a
    runtime.start_host_op(
        AsyncHostOp::Lstat {
            path: file_a_str.clone(),
        },
        true,
    );
    let res = next_host_completion(&mut runtime)
        .expect("lstat completes")
        .result;
    assert_eq!(
        res,
        Ok(AsyncHostValue::FileStat(FileStat {
            size: 11,
            is_file: true,
            is_directory: false,
            is_symlink: false,
        }))
    );

    // Unlink file_a
    runtime.start_host_op(
        AsyncHostOp::Unlink {
            path: file_a_str.clone(),
        },
        true,
    );
    let res = next_host_completion(&mut runtime)
        .expect("unlink completes")
        .result;
    assert_eq!(res, Ok(AsyncHostValue::Undefined));

    // Rm file_c
    runtime.start_host_op(
        AsyncHostOp::Rm {
            path: file_c_str.clone(),
            recursive: false,
            force: false,
        },
        true,
    );
    let res = next_host_completion(&mut runtime)
        .expect("rm completes")
        .result;
    assert_eq!(res, Ok(AsyncHostValue::Undefined));
}

#[test]
fn tokio_host_fast_fails_missing_files_and_stats_on_realm_thread() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    let missing_path = "/path/to/definitely/nonexistent/file.bin".to_string();

    let stat_req = runtime.start_host_op(
        AsyncHostOp::Stat {
            path: missing_path.clone(),
        },
        true,
    );
    let stat_comp = next_host_completion(&mut runtime).expect("stat completes");
    assert_eq!(stat_comp.request, stat_req);
    let stat_err = stat_comp.result.expect_err("missing file stat fails");
    assert_eq!(stat_err.code, "ENOENT");
    assert_eq!(stat_err.syscall, "stat");

    let read_req = runtime.start_host_op(AsyncHostOp::ReadFile { path: missing_path }, true);
    let read_comp = next_host_completion(&mut runtime).expect("read completes");
    assert_eq!(read_comp.request, read_req);
    let read_err = read_comp.result.expect_err("missing file read fails");
    assert_eq!(read_err.code, "ENOENT");
    assert_eq!(read_err.syscall, "open");
}

#[test]
fn tokio_host_maps_missing_paths_to_node_error_fields() {
    let directory = TestDirectory::new("async-missing");
    let path = directory.join("missing.txt").to_string_lossy().into_owned();
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(AsyncHostOp::ReadFile { path: path.clone() }, true);
    let error = next_host_completion(&mut runtime)
        .expect("read completes")
        .result
        .expect_err("missing path fails");
    assert_eq!(error.code, "ENOENT");
    assert_eq!(error.syscall, "open");
    assert_eq!(error.path.as_deref(), Some(path.as_str()));
}

#[test]
fn unreferenced_requests_do_not_keep_the_realm_alive() {
    let directory = TestDirectory::new("async-unref");
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::ReadFile {
            path: directory.join("missing").to_string_lossy().into_owned(),
        },
        false,
    );
    assert!(!runtime.host().has_referenced_work());
    assert!(next_host_completion(&mut runtime).is_some());
}

#[test]
fn cancellation_removes_request_and_liveness() {
    let directory = TestDirectory::new("async-cancel");
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    let request = runtime.start_host_op(
        AsyncHostOp::ReadFile {
            path: directory.join("missing").to_string_lossy().into_owned(),
        },
        true,
    );
    assert!(AsyncHostOps::cancel(runtime.host(), request));
    assert!(!AsyncHostOps::cancel(runtime.host(), request));
    assert!(!runtime.host().has_referenced_work());
    assert!(next_host_completion(&mut runtime).is_none());
}

#[test]
fn supervised_task_panic_fails_the_realm_and_cancels_siblings() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    let failed = RequestId::new(90);
    let sibling = RequestId::new(91);
    let executor = Rc::clone(&runtime.executor);
    let (local, tokio_runtime, host) = (
        &executor.local,
        &executor.reactor.runtime,
        &mut runtime.state.host,
    );
    local.block_on(tokio_runtime, async {
        host.spawn_task(failed, 90, true, async {
            panic!("supervised task panic");
        });
        host.spawn_task(sibling, 91, true, std::future::pending());
    });

    let failure = runtime
        .next_host_completion()
        .expect_err("task panic fails the realm");
    assert_eq!(failure.code, "ERR_KILN_RUNTIME_TASK_PANIC");
    assert!(failure.message.contains("supervised task panic"));
    assert!(!runtime.has_pending_host_work());
    assert!(!runtime.has_referenced_host_work());
    assert_eq!(
        runtime
            .next_host_completion()
            .expect_err("realm failure remains sticky"),
        failure
    );

    let mut realm: Realm = Realm::new();
    assert!(matches!(
        runtime.next_turn(&mut realm),
        Some(RuntimeTurn::Failure(observed)) if observed == failure
    ));
}

#[test]
fn runtime_metrics_are_exposed_as_a_kiln_owned_snapshot() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    let first = runtime.sample_runtime_metrics();
    assert_eq!(first.workers, 1);
    assert_eq!(first.global_queue_depth, 0);

    runtime.enter(|| {});
    let second = runtime.sample_runtime_metrics();
    assert_eq!(second.workers, 1);
    assert!(second.busy_duration <= second.elapsed);
}

#[test]
fn cancellation_retires_a_resource_in_an_undelivered_listen_completion() {
    let mut runtime = RealmRuntime::new().expect("runtime");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let listener = runtime.enter(|| tokio::net::TcpListener::from_std(listener).unwrap());
    let request = runtime.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        true,
    );
    let sequence = runtime.host_mut().pending[&request].sequence;
    runtime.host_mut().promote_completion(HostCompletion {
        request,
        sequence,
        lifecycle: None,
        result: Ok(HostCompletionValue::TcpListener {
            listener: HostListener::Tokio(listener),
            referenced: true,
        }),
    });
    assert_eq!(runtime.host_mut().resources.len(), 1);
    assert!(runtime.cancel_host_op(request));
    assert!(runtime.host_mut().resources.is_empty());
    assert!(AsyncHostOps::next_completion(runtime.host_mut()).is_none());
    assert!(!AsyncHostOps::has_referenced_work(runtime.host_mut()));
}

#[test]
fn cancellation_closes_an_undelivered_connected_or_accepted_socket() {
    use std::io::Read;
    for accepted in [false, true] {
        let mut runtime = RealmRuntime::new().expect("runtime");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let mut peer = std::net::TcpStream::connect(address).unwrap();
        peer.set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        let (stream, peer_address) = listener.accept().unwrap();
        stream.set_nonblocking(true).unwrap();
        let stream = runtime.enter(|| tokio::net::TcpStream::from_std(stream).unwrap());
        let request = runtime.start_host_op(
            AsyncHostOp::TcpConnect {
                host: "127.0.0.1".into(),
                port: address.port(),
            },
            true,
        );
        let sequence = runtime.host_mut().pending[&request].sequence;
        runtime.host_mut().promote_completion(HostCompletion {
            request,
            sequence,
            lifecycle: None,
            result: Ok(HostCompletionValue::TcpStream {
                stream,
                peer: peer_address,
                referenced: true,
                accepted,
                ticket: None,
            }),
        });
        assert_eq!(runtime.host_mut().resources.len(), 1);
        assert!(runtime.cancel_host_op(request));
        assert!(runtime.host_mut().resources.is_empty());
        assert!(!AsyncHostOps::has_referenced_work(runtime.host_mut()));
        assert_eq!(
            peer.read(&mut [0u8; 1]).unwrap(),
            0,
            "cancelled socket closes its peer"
        );
    }
}

#[test]
fn stale_completion_queued_after_cancellation_is_rejected() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    let request = runtime.start_host_op(
        AsyncHostOp::ReadFile {
            path: "canceled.txt".into(),
        },
        false,
    );
    assert!(runtime.cancel_host_op(request));
    runtime.host_mut().promote_completion(HostCompletion {
        request,
        sequence: 0,
        lifecycle: None,
        result: Ok(HostCompletionValue::Public(AsyncHostValue::Bytes(
            b"stale".to_vec(),
        ))),
    });

    assert!(AsyncHostOps::next_completion(runtime.host_mut()).is_none());
}

#[test]
fn stale_completion_does_not_end_a_later_turn() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    let stale = runtime.start_host_op(
        AsyncHostOp::ReadFile {
            path: "canceled.txt".into(),
        },
        false,
    );
    assert!(runtime.cancel_host_op(stale));
    runtime.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        false,
    );
    let listener = match next_host_completion(&mut runtime)
        .expect("listen completes")
        .result
        .expect("listen succeeds")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener, got {other:?}"),
    };
    let live = runtime.start_host_op(
        AsyncHostOp::TcpAccept {
            listener: listener.id,
        },
        false,
    );
    runtime.host_mut().promote_completion(HostCompletion {
        request: stale,
        sequence: 0,
        lifecycle: None,
        result: Ok(HostCompletionValue::Public(AsyncHostValue::Bytes(
            b"stale".to_vec(),
        ))),
    });
    let mut realm: Realm = Realm::new();
    realm.set_timeout(Duration::ZERO, WorkId::new(93));
    let turn = runtime
        .next_turn(&mut realm)
        .expect("stale completion must not end the drive");
    match turn {
        RuntimeTurn::Timer(WorkItem::Timer { work, .. }) => {
            assert_eq!(work, WorkId::new(93));
            assert!(runtime.cancel_host_op(live));
        }
        RuntimeTurn::Timer(WorkItem::Microtask(_)) => {
            panic!("next_turn must not select a microtask")
        }
        RuntimeTurn::Completion(_) => panic!("accept must remain pending without a client"),
        RuntimeTurn::Failure(failure) => panic!("realm failed unexpectedly: {failure}"),
        RuntimeTurn::Protocol => panic!("no HTTP engine queued anything here"),
    }
    runtime.start_host_op(
        AsyncHostOp::CloseResource {
            resource: listener.id,
        },
        false,
    );
    next_host_completion(&mut runtime).expect("close completes");
}

#[test]
fn persistent_resources_own_liveness_until_unref_or_close() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        true,
    );
    let resource = match next_host_completion(&mut runtime)
        .expect("listen completes")
        .result
        .expect("listen succeeds")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener, got {other:?}"),
    };
    assert_eq!(resource.kind, ResourceKind::TcpListener);
    assert!(runtime.host().has_referenced_work());

    runtime.start_host_op(
        AsyncHostOp::SetResourceReferenced {
            resource: resource.id,
            referenced: false,
        },
        true,
    );
    assert_eq!(
        next_host_completion(&mut runtime)
            .expect("unref completes")
            .result,
        Ok(AsyncHostValue::Undefined)
    );
    assert!(!runtime.host().has_referenced_work());

    runtime.start_host_op(
        AsyncHostOp::CloseResource {
            resource: resource.id,
        },
        true,
    );
    assert_eq!(
        next_host_completion(&mut runtime)
            .expect("close completes")
            .result,
        Ok(AsyncHostValue::Undefined)
    );
}

#[test]
fn closed_resource_id_fails_instead_of_aliasing_a_new_resource() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        false,
    );
    let resource = match next_host_completion(&mut runtime)
        .expect("listen completes")
        .result
        .expect("listen succeeds")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener, got {other:?}"),
    };
    runtime.start_host_op(
        AsyncHostOp::CloseResource {
            resource: resource.id,
        },
        true,
    );
    next_host_completion(&mut runtime).expect("first close completes");
    runtime.start_host_op(
        AsyncHostOp::CloseResource {
            resource: resource.id,
        },
        true,
    );
    let error = next_host_completion(&mut runtime)
        .expect("second close completes")
        .result
        .expect_err("closed resource is rejected");
    assert_eq!(error.code, "EBADF");
    assert_eq!(error.syscall, "close");
}

#[test]
fn scripted_host_validates_requests_and_releases_in_submission_order() {
    let first_op = AsyncHostOp::ReadFile {
        path: "fixture/a.txt".into(),
    };
    let second_op = AsyncHostOp::WriteFile {
        path: "fixture/b.txt".into(),
        data: b"written".to_vec(),
    };
    let mut host = ScriptedHost::new([
        ScriptedHostStep {
            operation: first_op.clone(),
            result: Ok(AsyncHostValue::Bytes(b"first".to_vec())),
        },
        ScriptedHostStep {
            operation: second_op.clone(),
            result: Ok(AsyncHostValue::Undefined),
        },
    ]);

    let first = host.start(first_op, true);
    let second = host.start(second_op, false);
    assert!(host.has_referenced_work());
    assert_eq!(host.remaining_steps(), 0);
    assert_eq!(
        host.next_completion().expect("first checkpoint").request,
        first
    );
    assert!(!host.has_referenced_work());
    assert_eq!(
        host.next_completion().expect("second checkpoint").request,
        second
    );
    assert!(host.is_exhausted());
}

#[test]
fn scripted_host_fails_closed_and_models_cancellation() {
    let mut host = ScriptedHost::new([ScriptedHostStep {
        operation: AsyncHostOp::ReadFile {
            path: "expected.txt".into(),
        },
        result: Ok(AsyncHostValue::Bytes(Vec::new())),
    }]);
    let mismatch = host.start(
        AsyncHostOp::ReadFile {
            path: "ambient.txt".into(),
        },
        true,
    );
    let error = host
        .next_completion()
        .expect("mismatch is released")
        .result
        .expect_err("unexpected machine access is rejected");
    assert_eq!(error.code, "ERR_KILN_UNEXPECTED_HOST_OP");
    assert!(error.message.contains("expected.txt"));
    assert!(error.message.contains("ambient.txt"));

    let unexpected = host.start(AsyncHostOp::Stat { path: "x".into() }, true);
    assert!(host.cancel(unexpected));
    assert!(!host.cancel(unexpected));
    assert!(host.next_completion().is_none());
    assert_ne!(mismatch, unexpected);
}

#[test]
fn scripted_runtime_drives_the_stream_driver_through_fragmented_reads_and_queued_writes() {
    use crate::node_stream::{StreamAction, StreamDriver, StreamEvent};

    let resource = ResourceId::new(41);
    let connect = AsyncHostOp::TcpConnect {
        host: "127.0.0.1".into(),
        port: 8080,
    };
    let first_read = AsyncHostOp::TcpRead {
        stream: resource,
        max_bytes: 4,
    };
    let second_read = first_read.clone();
    let third_read = first_read.clone();
    let first_write = AsyncHostOp::TcpWrite {
        stream: resource,
        data: b"abcd".to_vec(),
    };
    let second_write = AsyncHostOp::TcpWrite {
        stream: resource,
        data: b"ef".to_vec(),
    };
    let mut runtime = RealmRuntime::with_host(ScriptedHost::new([
        ScriptedHostStep {
            operation: connect.clone(),
            result: Ok(AsyncHostValue::Resource(ResourceInfo {
                id: resource,
                kind: ResourceKind::TcpStream,
                address: Some("127.0.0.1".into()),
                port: Some(8080),
            })),
        },
        ScriptedHostStep {
            operation: first_read.clone(),
            result: Ok(AsyncHostValue::Bytes(b"ab".to_vec())),
        },
        ScriptedHostStep {
            operation: second_read,
            result: Ok(AsyncHostValue::Bytes(b"cd".to_vec())),
        },
        ScriptedHostStep {
            operation: third_read,
            result: Ok(AsyncHostValue::Bytes(Vec::new())),
        },
        ScriptedHostStep {
            operation: first_write.clone(),
            result: Ok(AsyncHostValue::Undefined),
        },
        ScriptedHostStep {
            operation: second_write.clone(),
            result: Ok(AsyncHostValue::Undefined),
        },
    ]))
    .expect("scripted runtime builds");
    let connect_request = runtime.start_host_op(connect, true);
    assert_eq!(
        next_host_completion(&mut runtime)
            .expect("connect completes")
            .request,
        connect_request
    );
    let mut driver = StreamDriver::new(resource, 4);

    let actions = driver.start_reading();
    let StreamAction::Start(operation) = &actions[0] else {
        panic!("expected first scripted read, got {actions:?}");
    };
    assert_eq!(operation, &first_read);
    let request = runtime.start_host_op(operation.clone(), false);
    assert!(driver.started(operation, request));

    let actions = driver
        .completed(next_host_completion(&mut runtime).expect("first read completes"))
        .expect("first read belongs to stream");
    assert_eq!(actions[0], StreamAction::Data(b"ab".to_vec()));
    let StreamAction::Start(operation) = &actions[1] else {
        panic!("expected second scripted read, got {actions:?}");
    };
    let request = runtime.start_host_op(operation.clone(), false);
    assert!(driver.started(operation, request));

    let actions = driver
        .completed(next_host_completion(&mut runtime).expect("second read completes"))
        .expect("second read belongs to stream");
    assert_eq!(actions[0], StreamAction::Data(b"cd".to_vec()));
    let StreamAction::Start(operation) = &actions[1] else {
        panic!("expected EOF read, got {actions:?}");
    };
    let request = runtime.start_host_op(operation.clone(), false);
    assert!(driver.started(operation, request));
    assert_eq!(
        driver.completed(next_host_completion(&mut runtime).expect("EOF completes")),
        Some(vec![StreamAction::Event(StreamEvent::End)])
    );

    let (first, actions) = driver.write(b"abcd".to_vec());
    assert!(!first.ready);
    let StreamAction::Start(operation) = &actions[0] else {
        panic!("expected first scripted write, got {actions:?}");
    };
    assert_eq!(operation, &first_write);
    let request = runtime.start_host_op(operation.clone(), false);
    assert!(driver.started(operation, request));
    let (second, actions) = driver.write(b"ef".to_vec());
    assert!(!second.ready);
    assert!(actions.is_empty());

    let actions = driver
        .completed(next_host_completion(&mut runtime).expect("first write completes"))
        .expect("first write belongs to stream");
    assert_eq!(actions[0], StreamAction::Event(StreamEvent::Drain));
    let StreamAction::Start(operation) = &actions[1] else {
        panic!("expected queued scripted write, got {actions:?}");
    };
    assert_eq!(operation, &second_write);
    let request = runtime.start_host_op(operation.clone(), false);
    assert!(driver.started(operation, request));
    assert!(
        driver
            .completed(next_host_completion(&mut runtime).expect("second write completes"))
            .expect("second write belongs to stream")
            .is_empty()
    );
    assert!(runtime.host_mut().is_exhausted());
}

#[test]
fn scripted_runtime_cancellation_retires_stream_read_before_close() {
    use crate::node_stream::{StreamAction, StreamDriver, StreamEvent};

    let resource = ResourceId::new(42);
    let read = AsyncHostOp::TcpRead {
        stream: resource,
        max_bytes: 8,
    };
    let close = AsyncHostOp::CloseResource { resource };
    let mut runtime = RealmRuntime::with_host(ScriptedHost::new([
        ScriptedHostStep {
            operation: read.clone(),
            result: Ok(AsyncHostValue::Bytes(b"late".to_vec())),
        },
        ScriptedHostStep {
            operation: close.clone(),
            result: Ok(AsyncHostValue::Undefined),
        },
    ]))
    .expect("scripted runtime builds");
    let mut driver = StreamDriver::new(resource, 8);
    let actions = driver.start_reading();
    let StreamAction::Start(operation) = &actions[0] else {
        panic!("expected scripted read, got {actions:?}");
    };
    let request = runtime.start_host_op(operation.clone(), false);
    assert!(driver.started(operation, request));

    let actions = driver.close_actions();
    assert_eq!(actions[0], StreamAction::Cancel(request));
    let StreamAction::Start(close_operation) = &actions[1] else {
        panic!("expected close action, got {actions:?}");
    };
    assert_eq!(close_operation, &close);
    assert!(runtime.cancel_host_op(request));
    assert!(driver.cancelled(request));
    let close_request = runtime.start_host_op(close_operation.clone(), false);
    assert!(driver.started(close_operation, close_request));
    assert_eq!(
        driver.completed(next_host_completion(&mut runtime).expect("close completes")),
        Some(vec![StreamAction::Event(StreamEvent::Close)])
    );
    assert!(runtime.host_mut().is_exhausted());
}

#[test]
fn scripted_runtime_keeps_equal_checkpoint_completions_in_submission_order() {
    let first_op = AsyncHostOp::ReadFile {
        path: "scripted/first".into(),
    };
    let second_op = AsyncHostOp::ReadFile {
        path: "scripted/second".into(),
    };
    let mut runtime = RealmRuntime::with_host(ScriptedHost::new([
        ScriptedHostStep {
            operation: first_op.clone(),
            result: Ok(AsyncHostValue::Bytes(b"first".to_vec())),
        },
        ScriptedHostStep {
            operation: second_op.clone(),
            result: Ok(AsyncHostValue::Bytes(b"second".to_vec())),
        },
    ]))
    .expect("scripted runtime builds");
    let mut realm = Realm::<(), ()>::new();
    realm.set_timeout(Duration::ZERO, ());
    let first = runtime.start_host_op(first_op, false);
    let second = runtime.start_host_op(second_op, false);
    let first_completion = match runtime.next_turn(&mut realm).expect("first completion") {
        RuntimeTurn::Completion(completion) => completion,
        RuntimeTurn::Timer(_) => panic!("a ready completion must beat a due timer"),
        RuntimeTurn::Failure(failure) => panic!("realm failed unexpectedly: {failure}"),
        RuntimeTurn::Protocol => panic!("no HTTP engine queued anything here"),
    };
    let second_completion = match runtime.next_turn(&mut realm).expect("second completion") {
        RuntimeTurn::Completion(completion) => completion,
        RuntimeTurn::Timer(_) => panic!("a ready completion must beat a due timer"),
        RuntimeTurn::Failure(failure) => panic!("realm failed unexpectedly: {failure}"),
        RuntimeTurn::Protocol => panic!("no HTTP engine queued anything here"),
    };
    assert_eq!(first_completion.request, first);
    assert_eq!(second_completion.request, second);
    assert!(first_completion.sequence < second_completion.sequence);
    assert!(runtime.host_mut().is_exhausted());
}

#[test]
fn dropping_realm_runtime_uses_the_idempotent_shutdown_protocol() {
    let observer = Rc::new(std::cell::Cell::new(0));
    let host = ScriptedHost {
        shutdown_observer: Some(Rc::clone(&observer)),
        ..ScriptedHost::default()
    };
    {
        let mut runtime = RealmRuntime::with_host(host).expect("scripted runtime builds");
        assert!(runtime.shutdown());
        assert!(!runtime.shutdown());
    }
    assert_eq!(observer.get(), 1, "Drop must reuse, not repeat, shutdown");

    let host = ScriptedHost {
        shutdown_observer: Some(Rc::clone(&observer)),
        ..ScriptedHost::default()
    };
    drop(RealmRuntime::with_host(host).expect("scripted runtime builds"));
    assert_eq!(observer.get(), 2, "Drop must initiate shutdown when needed");
}

#[test]
fn tokio_tcp_resources_exchange_bytes_and_shutdown_cleanly() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        true,
    );
    let listener = match next_host_completion(&mut runtime)
        .expect("listen completes")
        .result
        .expect("listen succeeds")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener resource, got {other:?}"),
    };
    assert_eq!(listener.kind, ResourceKind::TcpListener);
    assert_eq!(listener.address.as_deref(), Some("127.0.0.1"));
    let port = listener.port.expect("ephemeral port assigned");
    assert_ne!(port, 0);

    let accept = runtime.start_host_op(
        AsyncHostOp::TcpAccept {
            listener: listener.id,
        },
        true,
    );
    let connect = runtime.start_host_op(
        AsyncHostOp::TcpConnect {
            host: "127.0.0.1".into(),
            port,
        },
        true,
    );
    let first = next_host_completion(&mut runtime).expect("connection completes");
    let second = next_host_completion(&mut runtime).expect("accept completes");
    let mut accepted = None;
    let mut client = None;
    for completion in [first, second] {
        if completion.request == accept {
            accepted = Some(match completion.result.expect("accept succeeds") {
                AsyncHostValue::AcceptedTcpStream(accepted) => accepted,
                other => panic!("expected accepted stream, got {other:?}"),
            });
        } else if completion.request == connect {
            client = Some(match completion.result.expect("connect succeeds") {
                AsyncHostValue::Resource(resource) => resource,
                other => panic!("expected connected stream, got {other:?}"),
            });
        } else {
            panic!("unexpected connection request");
        }
    }
    let accepted = accepted.expect("accept result observed");
    let client = client.expect("connect result observed");
    assert_eq!(accepted.resource.kind, ResourceKind::TcpStream);
    assert_eq!(client.kind, ResourceKind::TcpStream);
    assert_eq!(accepted.peer_address, "127.0.0.1");

    runtime.start_host_op(
        AsyncHostOp::TcpWrite {
            stream: client.id,
            data: b"hello kiln".to_vec(),
        },
        true,
    );
    assert_eq!(
        next_host_completion(&mut runtime)
            .expect("write completes")
            .result,
        Ok(AsyncHostValue::Undefined)
    );
    runtime.start_host_op(
        AsyncHostOp::TcpRead {
            stream: accepted.resource.id,
            max_bytes: 64,
        },
        true,
    );
    assert_eq!(
        next_host_completion(&mut runtime)
            .expect("read completes")
            .result,
        Ok(AsyncHostValue::Bytes(b"hello kiln".to_vec()))
    );

    runtime.start_host_op(AsyncHostOp::TcpShutdown { stream: client.id }, true);
    next_host_completion(&mut runtime).expect("shutdown completes");
    runtime.start_host_op(
        AsyncHostOp::TcpRead {
            stream: accepted.resource.id,
            max_bytes: 64,
        },
        true,
    );
    assert_eq!(
        next_host_completion(&mut runtime)
            .expect("eof completes")
            .result,
        Ok(AsyncHostValue::Bytes(Vec::new()))
    );

    for resource in [client.id, accepted.resource.id, listener.id] {
        runtime.start_host_op(AsyncHostOp::CloseResource { resource }, true);
        assert_eq!(
            next_host_completion(&mut runtime)
                .expect("close completes")
                .result,
            Ok(AsyncHostValue::Undefined)
        );
    }
    assert!(!runtime.host().has_referenced_work());
}

/// With the io_uring backend a listener is the ring's, its connections arrive raw, and a
/// discrete read or write gives one its Tokio halves on demand.
#[cfg(target_os = "linux")]
#[test]
fn uring_listener_accepts_raw_streams_that_serve_discrete_io() {
    let mut host = TokioHost::new();
    host.set_io_preference(IoPreference::Uring);
    let mut runtime = RealmRuntime::with_host(host).expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        true,
    );
    let listener = match next_host_completion(&mut runtime)
        .expect("listen completes")
        .result
        .expect("listen succeeds")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener resource, got {other:?}"),
    };
    let on_ring = matches!(
        runtime.host().resources.get(&listener.id),
        Some(HostResource::TcpListener { listener, .. })
            if matches!(**listener, HostListener::Ring(_))
    );
    if !on_ring {
        eprintln!("skipped: io_uring unavailable, the listener fell back to epoll");
        return;
    }
    let port = listener.port.expect("port");
    let accept = runtime.start_host_op(
        AsyncHostOp::TcpAccept {
            listener: listener.id,
        },
        true,
    );
    let mut peer = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let accepted = next_host_completion(&mut runtime).expect("accept completes");
    assert_eq!(accepted.request, accept);
    let AsyncHostValue::AcceptedTcpStream(accepted) = accepted.result.expect("accepted") else {
        panic!("expected an accepted stream");
    };
    assert_eq!(accepted.peer_address, "127.0.0.1");
    assert_eq!(accepted.peer_port, peer.local_addr().unwrap().port());
    assert_eq!(accepted.resource.port, Some(port));
    assert!(matches!(
        runtime.host().resources.get(&accepted.resource.id),
        Some(HostResource::TcpRaw { .. })
    ));
    std::io::Write::write_all(&mut peer, b"raw bytes").unwrap();
    runtime.start_host_op(
        AsyncHostOp::TcpRead {
            stream: accepted.resource.id,
            max_bytes: 64,
        },
        true,
    );
    assert_eq!(
        next_host_completion(&mut runtime).expect("read").result,
        Ok(AsyncHostValue::Bytes(b"raw bytes".to_vec()))
    );
    assert!(matches!(
        runtime.host().resources.get(&accepted.resource.id),
        Some(HostResource::TcpStream { .. })
    ));
    runtime.start_host_op(
        AsyncHostOp::TcpWrite {
            stream: accepted.resource.id,
            data: b"back".to_vec(),
        },
        true,
    );
    next_host_completion(&mut runtime).expect("write");
    let mut back = [0_u8; 4];
    std::io::Read::read_exact(&mut peer, &mut back).unwrap();
    assert_eq!(&back, b"back");
    for resource in [accepted.resource.id, listener.id] {
        runtime.start_host_op(AsyncHostOp::CloseResource { resource }, true);
        next_host_completion(&mut runtime).expect("close");
    }
    // The listener is really closed once the ring cancels its accept.
    Rc::clone(runtime.executor()).block_on(async {
        tokio::time::sleep(Duration::from_millis(20)).await;
    });
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
    assert!(runtime.shutdown());
}

#[test]
fn tcp_listeners_support_reuseport_across_runtimes() {
    let mut runtime1 = RealmRuntime::new().expect("runtime 1");
    let mut runtime2 = RealmRuntime::new().expect("runtime 2");
    runtime1.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        true,
    );
    let listener1 = match next_host_completion(&mut runtime1)
        .expect("listen 1 completes")
        .result
        .expect("listen 1 succeeds")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener resource, got {other:?}"),
    };
    let port = listener1.port.expect("port");

    // runtime2 listens on the exact same port thanks to SO_REUSEPORT
    runtime2.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port,
        },
        true,
    );
    let listener2 = match next_host_completion(&mut runtime2)
        .expect("listen 2 completes")
        .result
        .expect("listen 2 succeeds with reuseport")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener resource, got {other:?}"),
    };
    assert_eq!(listener2.port, Some(port));
}

#[test]
fn pending_socket_read_does_not_block_a_write_on_the_same_stream() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        false,
    );
    let listener = match next_host_completion(&mut runtime)
        .expect("listen completes")
        .result
        .expect("listen succeeds")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener, got {other:?}"),
    };
    let accept = runtime.start_host_op(
        AsyncHostOp::TcpAccept {
            listener: listener.id,
        },
        false,
    );
    let connect = runtime.start_host_op(
        AsyncHostOp::TcpConnect {
            host: "127.0.0.1".into(),
            port: listener.port.expect("ephemeral port assigned"),
        },
        false,
    );
    let mut server = None;
    let mut client = None;
    for _ in 0..2 {
        let completion = next_host_completion(&mut runtime).expect("connection completes");
        if completion.request == accept {
            server = Some(match completion.result.expect("accept succeeds") {
                AsyncHostValue::AcceptedTcpStream(accepted) => accepted.resource,
                other => panic!("expected accepted stream, got {other:?}"),
            });
        } else if completion.request == connect {
            client = Some(match completion.result.expect("connect succeeds") {
                AsyncHostValue::Resource(resource) => resource,
                other => panic!("expected connected stream, got {other:?}"),
            });
        }
    }
    let server = server.expect("accepted stream observed");
    let client = client.expect("connected stream observed");

    let pending_read = runtime.start_host_op(
        AsyncHostOp::TcpRead {
            stream: server.id,
            max_bytes: 64,
        },
        false,
    );
    assert!(!runtime.host().has_referenced_work());
    let mut realm = Realm::<(), ()>::new();
    assert!(runtime.next_referenced_turn(&mut realm).is_none());
    let response_write = runtime.start_host_op(
        AsyncHostOp::TcpWrite {
            stream: server.id,
            data: b"response while reading".to_vec(),
        },
        false,
    );
    let completion =
        next_host_completion(&mut runtime).expect("write completes without waiting for read EOF");
    assert_eq!(completion.request, response_write);
    assert_eq!(completion.result, Ok(AsyncHostValue::Undefined));

    runtime.start_host_op(
        AsyncHostOp::TcpRead {
            stream: client.id,
            max_bytes: 64,
        },
        false,
    );
    assert_eq!(
        next_host_completion(&mut runtime)
            .expect("peer reads response")
            .result,
        Ok(AsyncHostValue::Bytes(b"response while reading".to_vec()))
    );

    assert!(runtime.cancel_host_op(pending_read));
    for resource in [server.id, client.id, listener.id] {
        runtime.start_host_op(AsyncHostOp::CloseResource { resource }, false);
        next_host_completion(&mut runtime).expect("close completes");
    }
}

fn resource_state(runtime: &mut RealmRuntime, resource: ResourceId) -> ResourceState {
    runtime.start_host_op(AsyncHostOp::ResourceState { resource }, false);
    match next_host_completion(runtime)
        .expect("state completes")
        .result
        .expect("state succeeds")
    {
        AsyncHostValue::ResourceState(state) => state,
        other => panic!("expected resource state, got {other:?}"),
    }
}

#[test]
fn resource_state_is_the_single_source_of_stream_lifecycle_truth() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        true,
    );
    let listener = match next_host_completion(&mut runtime)
        .expect("listen completes")
        .result
        .expect("listen succeeds")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener, got {other:?}"),
    };
    let listener_state = resource_state(&mut runtime, listener.id);
    assert_eq!(listener_state.info, listener);
    assert!(listener_state.referenced);
    assert!(!listener_state.read_closed);
    assert!(!listener_state.write_closed);

    runtime.start_host_op(
        AsyncHostOp::SetResourceReferenced {
            resource: listener.id,
            referenced: false,
        },
        false,
    );
    next_host_completion(&mut runtime).expect("unref completes");
    assert!(!resource_state(&mut runtime, listener.id).referenced);

    runtime.start_host_op(
        AsyncHostOp::CloseResource {
            resource: listener.id,
        },
        false,
    );
    next_host_completion(&mut runtime).expect("close completes");
}

#[test]
fn stream_half_close_is_idempotent_and_persistently_observable() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        false,
    );
    let listener = match next_host_completion(&mut runtime)
        .expect("listen completes")
        .result
        .expect("listen succeeds")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener, got {other:?}"),
    };
    let port = listener.port.expect("ephemeral port assigned");
    let accept_request = runtime.start_host_op(
        AsyncHostOp::TcpAccept {
            listener: listener.id,
        },
        false,
    );
    let connect_request = runtime.start_host_op(
        AsyncHostOp::TcpConnect {
            host: "127.0.0.1".into(),
            port,
        },
        false,
    );
    let mut accepted = None;
    let mut client = None;
    for _ in 0..2 {
        let completion =
            next_host_completion(&mut runtime).expect("connection operation completes");
        if completion.request == accept_request {
            accepted = Some(match completion.result.expect("accept succeeds") {
                AsyncHostValue::AcceptedTcpStream(accepted) => accepted.resource,
                other => panic!("expected accepted stream, got {other:?}"),
            });
        } else if completion.request == connect_request {
            client = Some(match completion.result.expect("connect succeeds") {
                AsyncHostValue::Resource(resource) => resource,
                other => panic!("expected connected stream, got {other:?}"),
            });
        } else {
            panic!("unexpected connection request");
        }
    }
    let accepted = accepted.expect("accept result observed");
    let client = client.expect("connect result observed");

    for _ in 0..2 {
        runtime.start_host_op(AsyncHostOp::TcpShutdown { stream: client.id }, false);
        assert_eq!(
            next_host_completion(&mut runtime)
                .expect("shutdown completes")
                .result,
            Ok(AsyncHostValue::Undefined)
        );
    }
    assert!(resource_state(&mut runtime, client.id).write_closed);
    runtime.start_host_op(
        AsyncHostOp::TcpWrite {
            stream: client.id,
            data: b"too late".to_vec(),
        },
        false,
    );
    let error = next_host_completion(&mut runtime)
        .expect("write completes")
        .result
        .expect_err("write after end is rejected");
    assert_eq!(error.code, "EPIPE");

    runtime.start_host_op(
        AsyncHostOp::TcpRead {
            stream: accepted.id,
            max_bytes: 32,
        },
        false,
    );
    assert_eq!(
        next_host_completion(&mut runtime)
            .expect("EOF completes")
            .result,
        Ok(AsyncHostValue::Bytes(Vec::new()))
    );
    assert!(resource_state(&mut runtime, accepted.id).read_closed);
    runtime.start_host_op(
        AsyncHostOp::TcpRead {
            stream: accepted.id,
            max_bytes: 32,
        },
        false,
    );
    assert_eq!(
        next_host_completion(&mut runtime)
            .expect("cached EOF completes")
            .result,
        Ok(AsyncHostValue::Bytes(Vec::new()))
    );

    for resource in [client.id, accepted.id, listener.id] {
        runtime.start_host_op(AsyncHostOp::CloseResource { resource }, false);
        next_host_completion(&mut runtime).expect("close completes");
    }
}

#[test]
fn node_stream_driver_runs_the_same_action_loop_on_live_tokio_sockets() {
    use crate::node_stream::{StreamAction, StreamDriver, StreamEvent};

    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        false,
    );
    let listener = match next_host_completion(&mut runtime)
        .expect("listen completes")
        .result
        .expect("listen succeeds")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener, got {other:?}"),
    };
    let port = listener.port.expect("ephemeral port assigned");
    let accept = runtime.start_host_op(
        AsyncHostOp::TcpAccept {
            listener: listener.id,
        },
        false,
    );
    let connect = runtime.start_host_op(
        AsyncHostOp::TcpConnect {
            host: "127.0.0.1".into(),
            port,
        },
        false,
    );
    let mut accepted = None;
    let mut client = None;
    for _ in 0..2 {
        let completion = next_host_completion(&mut runtime).expect("connection completes");
        if completion.request == accept {
            accepted = Some(match completion.result.expect("accept succeeds") {
                AsyncHostValue::AcceptedTcpStream(accepted) => accepted.resource,
                other => panic!("expected accepted stream, got {other:?}"),
            });
        } else if completion.request == connect {
            client = Some(match completion.result.expect("connect succeeds") {
                AsyncHostValue::Resource(resource) => resource,
                other => panic!("expected client stream, got {other:?}"),
            });
        }
    }
    let accepted = accepted.expect("accepted stream observed");
    let client = client.expect("client stream observed");
    let mut reader = StreamDriver::new(accepted.id, 16);
    let mut writer = StreamDriver::new(client.id, 4);

    let read = reader.start_reading().pop().expect("read action");
    let StreamAction::Start(read_op) = &read else {
        panic!("expected read operation");
    };
    let read_request = runtime.start_host_op(read_op.clone(), false);
    assert!(reader.started(read_op, read_request));

    let (_, actions) = writer.end(Some(b"hello".to_vec()));
    assert_eq!(actions.len(), 1);
    let StreamAction::Start(write_op) = &actions[0] else {
        panic!("expected write operation");
    };
    let write_request = runtime.start_host_op(write_op.clone(), false);
    assert!(writer.started(write_op, write_request));

    let mut bytes = Vec::new();
    let mut writer_events = Vec::new();
    let mut reader_events = Vec::new();
    while !writer.state().write_closed() || !reader.state().read_closed() {
        let completion = next_host_completion(&mut runtime).expect("stream work completes");
        let (owner_is_writer, actions) = if let Some(actions) = writer.completed(completion.clone())
        {
            (true, actions)
        } else {
            (
                false,
                reader
                    .completed(completion)
                    .expect("completion belongs to a driver"),
            )
        };
        for action in actions {
            match action {
                StreamAction::Start(operation) => {
                    let request = runtime.start_host_op(operation.clone(), false);
                    assert!(
                        writer.started(&operation, request) || reader.started(&operation, request)
                    );
                }
                StreamAction::Cancel(request) => {
                    panic!("unexpected cancellation of live stream request {request:?}");
                }
                StreamAction::Data(data) => bytes.extend(data),
                StreamAction::Event(event) => {
                    if owner_is_writer {
                        writer_events.push(event);
                    } else {
                        reader_events.push(event);
                    }
                }
                StreamAction::Error(error) => panic!("unexpected stream error: {error}"),
            }
        }
    }
    assert_eq!(bytes, b"hello");
    assert_eq!(writer_events, [StreamEvent::Drain, StreamEvent::Finish]);
    assert_eq!(reader_events, [StreamEvent::End]);

    for resource in [client.id, accepted.id, listener.id] {
        runtime.start_host_op(AsyncHostOp::CloseResource { resource }, false);
        next_host_completion(&mut runtime).expect("close completes");
    }
}

#[test]
fn node_server_driver_rearms_accept_on_live_tokio_listener() {
    use crate::node_net::{ServerAction, TcpServerDriver};

    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    let mut server = TcpServerDriver::default();
    let ServerAction::Start(listen) = server.listen("127.0.0.1".into(), 0) else {
        unreachable!()
    };
    let listen_request = runtime.start_host_op(listen.clone(), false);
    assert!(server.started(&listen, listen_request));
    let actions = server
        .completed(next_host_completion(&mut runtime).expect("listen completes"))
        .expect("listen belongs to server");
    let listener = match &actions[0] {
        ServerAction::Listening(listener) => listener.clone(),
        other => panic!("expected listening action, got {other:?}"),
    };
    let ServerAction::Start(accept) = &actions[1] else {
        panic!("expected accept action");
    };
    let accept_request = runtime.start_host_op(accept.clone(), false);
    assert!(server.started(accept, accept_request));

    let port = listener.port.expect("ephemeral port assigned");
    runtime.start_host_op(
        AsyncHostOp::TcpConnect {
            host: "127.0.0.1".into(),
            port,
        },
        false,
    );
    let mut accepted_resource = None;
    let mut client_resource = None;
    while accepted_resource.is_none() || client_resource.is_none() {
        let completion = next_host_completion(&mut runtime).expect("connection completes");
        if completion.request == accept_request {
            let actions = server
                .completed(completion)
                .expect("accept belongs to server");
            match &actions[0] {
                ServerAction::Connection(accepted) => {
                    accepted_resource = Some(accepted.resource.id)
                }
                other => panic!("expected connection, got {other:?}"),
            }
            let ServerAction::Start(next_accept) = &actions[1] else {
                panic!("expected rearmed accept");
            };
            let next_request = runtime.start_host_op(next_accept.clone(), false);
            assert!(server.started(next_accept, next_request));
            assert!(AsyncHostOps::cancel(runtime.host(), next_request));
        } else {
            client_resource = Some(match completion.result.expect("connect succeeds") {
                AsyncHostValue::Resource(resource) => resource.id,
                other => panic!("expected client stream, got {other:?}"),
            });
        }
    }

    let close_actions = server.close();
    let ServerAction::Start(close) = close_actions.last().expect("close starts") else {
        unreachable!()
    };
    let close_request = runtime.start_host_op(close.clone(), false);
    assert!(server.started(close, close_request));
    assert_eq!(
        server.completed(next_host_completion(&mut runtime).expect("close completes")),
        Some(vec![ServerAction::Close])
    );
    for resource in [accepted_resource.unwrap(), client_resource.unwrap()] {
        runtime.start_host_op(AsyncHostOp::CloseResource { resource }, false);
        next_host_completion(&mut runtime).expect("stream close completes");
    }
}

#[test]
fn realm_shutdown_cancels_pending_tcp_work_before_closing_resources() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        true,
    );
    let listener = match next_host_completion(&mut runtime)
        .expect("listen completes")
        .result
        .expect("listen succeeds")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener, got {other:?}"),
    };

    let accept_request = runtime.start_host_op(
        AsyncHostOp::TcpAccept {
            listener: listener.id,
        },
        true,
    );
    let connect_request = runtime.start_host_op(
        AsyncHostOp::TcpConnect {
            host: "127.0.0.1".into(),
            port: listener.port.expect("ephemeral port assigned"),
        },
        true,
    );
    let mut accepted = None;
    let mut client = None;
    while accepted.is_none() || client.is_none() {
        let completion = next_host_completion(&mut runtime).expect("connection completes");
        if completion.request == accept_request {
            accepted = Some(match completion.result.expect("accept succeeds") {
                AsyncHostValue::AcceptedTcpStream(stream) => stream.resource.id,
                other => panic!("expected accepted stream, got {other:?}"),
            });
        } else {
            assert_eq!(completion.request, connect_request);
            client = Some(match completion.result.expect("connect succeeds") {
                AsyncHostValue::Resource(resource) => resource.id,
                other => panic!("expected client stream, got {other:?}"),
            });
        }
    }

    let pending_accept = runtime.start_host_op(
        AsyncHostOp::TcpAccept {
            listener: listener.id,
        },
        true,
    );
    let pending_read = runtime.start_host_op(
        AsyncHostOp::TcpRead {
            stream: accepted.expect("accepted resource"),
            max_bytes: 16,
        },
        true,
    );
    assert_ne!(pending_accept, pending_read);
    assert_eq!(runtime.host.pending.len(), 2);
    assert_eq!(runtime.host.resources.len(), 3);

    assert!(runtime.shutdown());
    assert!(runtime.host.shutdown);
    assert!(runtime.host.pending.is_empty());
    assert!(runtime.host.ready.is_empty());
    assert!(runtime.host.resources.is_empty());
    assert!(runtime.host.tasks.is_empty());

    runtime
        .executor
        .local
        .block_on(&runtime.executor.reactor.runtime, async {
            tokio::task::yield_now().await;
        });
    assert_eq!(
        runtime
            .next_host_completion()
            .expect("late task exits are discarded"),
        None
    );
    assert!(!runtime.shutdown());
    let _ = client;
}

#[test]
fn http_core_composes_with_stream_driver_for_a_live_tokio_exchange() {
    use crate::node_http::{Http1Decoder, HttpResponse};

    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        false,
    );
    let listener = match next_host_completion(&mut runtime)
        .expect("listen completes")
        .result
        .expect("listen succeeds")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener, got {other:?}"),
    };
    let accept = runtime.start_host_op(
        AsyncHostOp::TcpAccept {
            listener: listener.id,
        },
        false,
    );
    let connect = runtime.start_host_op(
        AsyncHostOp::TcpConnect {
            host: "127.0.0.1".into(),
            port: listener.port.expect("ephemeral port assigned"),
        },
        false,
    );
    let mut server_stream = None;
    let mut client_stream = None;
    for _ in 0..2 {
        let completion = next_host_completion(&mut runtime).expect("connection completes");
        if completion.request == accept {
            server_stream = Some(match completion.result.expect("accept succeeds") {
                AsyncHostValue::AcceptedTcpStream(accepted) => accepted.resource,
                other => panic!("expected accepted stream, got {other:?}"),
            });
        } else if completion.request == connect {
            client_stream = Some(match completion.result.expect("connect succeeds") {
                AsyncHostValue::Resource(resource) => resource,
                other => panic!("expected client stream, got {other:?}"),
            });
        }
    }
    let server_stream = server_stream.unwrap();
    let client_stream = client_stream.unwrap();

    runtime.start_host_op(
            AsyncHostOp::TcpWrite {
                stream: client_stream.id,
                data: b"POST /echo HTTP/1.1\r\nHost: kiln\r\nContent-Length: 4\r\nConnection: close\r\n\r\nping".to_vec(),
            },
            false,
        );
    next_host_completion(&mut runtime).expect("request write completes");
    runtime.start_host_op(
        AsyncHostOp::TcpRead {
            stream: server_stream.id,
            max_bytes: 4096,
        },
        false,
    );
    let request_bytes = match next_host_completion(&mut runtime)
        .expect("request read completes")
        .result
        .expect("request read succeeds")
    {
        AsyncHostValue::Bytes(bytes) => bytes,
        other => panic!("expected request bytes, got {other:?}"),
    };
    let request = Http1Decoder::default()
        .push(&request_bytes)
        .expect("request parses")
        .pop()
        .expect("complete request");
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/echo");
    assert_eq!(request.body, b"ping");

    let response = HttpResponse {
        status: 200,
        reason: None,
        headers: vec![("Content-Type".into(), "text/plain".into())],
        body: bytes::Bytes::from(request.body),
    }
    .encode(request.keep_alive);
    runtime.start_host_op(
        AsyncHostOp::TcpWrite {
            stream: server_stream.id,
            data: response,
        },
        false,
    );
    next_host_completion(&mut runtime).expect("response write completes");
    runtime.start_host_op(
        AsyncHostOp::TcpShutdown {
            stream: server_stream.id,
        },
        false,
    );
    next_host_completion(&mut runtime).expect("response shutdown completes");
    runtime.start_host_op(
        AsyncHostOp::TcpRead {
            stream: client_stream.id,
            max_bytes: 4096,
        },
        false,
    );
    let response = match next_host_completion(&mut runtime)
        .expect("response read completes")
        .result
        .expect("response read succeeds")
    {
        AsyncHostValue::Bytes(bytes) => bytes,
        other => panic!("expected response bytes, got {other:?}"),
    };
    assert_eq!(
            response,
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\nConnection: close\r\n\r\nping"
        );

    for resource in [server_stream.id, client_stream.id, listener.id] {
        runtime.start_host_op(AsyncHostOp::CloseResource { resource }, false);
        next_host_completion(&mut runtime).expect("close completes");
    }
}

fn drive(realm: &mut Realm, events: &Rc<RefCell<Vec<String>>>) {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime
        .drive(
            realm,
            |realm, work| {
                match work {
                    WorkItem::Microtask(id) => {
                        events.borrow_mut().push(format!("micro:{}", id.get()));
                        if id.get() == 1 {
                            // This is appended after the already queued work, proving the
                            // queue is FIFO rather than depth-first.
                            realm.queue_microtask(WorkId::new(3));
                        }
                    }
                    WorkItem::Timer { work, .. } => events.borrow_mut().push(format!(
                        "timer:{}@{}",
                        work.get(),
                        realm.now().as_millis()
                    )),
                }
                Ok::<_, ()>(())
            },
            |_| Ok::<_, ()>(()),
        )
        .expect("realm drive succeeds");
}

#[test]
fn drains_fifo_microtasks_before_timers_and_between_callbacks() {
    let mut realm: Realm = Realm::new();
    let events = Rc::new(RefCell::new(Vec::new()));
    realm.queue_microtask(WorkId::new(1));
    realm.queue_microtask(WorkId::new(2));
    realm.set_timeout(Duration::from_millis(10), WorkId::new(4));
    realm.set_timeout(Duration::from_millis(10), WorkId::new(5));
    realm.set_timeout(Duration::from_millis(5), WorkId::new(6));

    drive(&mut realm, &events);

    assert_eq!(
        *events.borrow(),
        [
            "micro:1",
            "micro:2",
            "micro:3",
            "timer:6@5",
            "timer:4@10",
            "timer:5@10",
        ]
    );
    assert_eq!(realm.now(), Duration::from_millis(10));
    assert!(!realm.has_pending_work());
}

#[test]
fn generic_drive_drains_next_ticks_before_microtasks() {
    let mut realm: Realm = Realm::new();
    realm.queue_next_tick(WorkId::new(1));
    realm.queue_microtask(WorkId::new(2));
    let mut events = Vec::new();
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");

    runtime
        .drive(
            &mut realm,
            |_realm, work| {
                if let WorkItem::Microtask(work) = work {
                    events.push(work.get());
                }
                Ok::<_, ()>(())
            },
            |_| Ok::<_, ()>(()),
        )
        .expect("realm drive succeeds");

    assert_eq!(events, [1, 2]);
    assert!(!realm.has_pending_work());
}

#[test]
fn timer_callback_microtasks_run_before_the_next_timer() {
    let mut realm: Realm = Realm::new();
    let events = Rc::new(RefCell::new(Vec::new()));
    realm.set_timeout(Duration::from_millis(1), WorkId::new(1));
    realm.set_timeout(Duration::from_millis(1), WorkId::new(2));

    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime
        .drive(
            &mut realm,
            |realm, work| {
                if let WorkItem::Timer { work, .. } = work {
                    events.borrow_mut().push(format!("timer:{}", work.get()));
                    realm.queue_microtask(WorkId::new(work.get() + 10));
                } else if let WorkItem::Microtask(id) = work {
                    events.borrow_mut().push(format!("micro:{}", id.get()));
                }
                Ok::<_, ()>(())
            },
            |_| Ok::<_, ()>(()),
        )
        .expect("realm drive succeeds");

    assert_eq!(
        *events.borrow(),
        ["timer:1", "micro:11", "timer:2", "micro:12"]
    );
}

#[test]
fn interval_rearms_before_callback_and_can_be_cleared() {
    let mut realm: Realm = Realm::new();
    let interval = realm.set_interval(Duration::from_millis(3), WorkId::new(7));
    let mut runs = 0;
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime
        .drive(
            &mut realm,
            |realm, work| {
                if let WorkItem::Timer { id, .. } = work {
                    assert_eq!(id, interval);
                    runs += 1;
                    if runs == 3 {
                        assert!(realm.clear_timeout(id));
                    }
                }
                Ok::<_, ()>(())
            },
            |_| Ok::<_, ()>(()),
        )
        .expect("realm drive succeeds");

    assert_eq!(runs, 3);
    assert_eq!(realm.now(), Duration::from_millis(9));
    assert!(!realm.has_pending_work());
}

/// `Timeout.prototype.refresh()`: the same timer restarted with its delay from the realm's
/// current virtual time, still the same identity and still unreferenced if it was; a finished or
/// cleared timer is not re-armed.
#[test]
fn refreshed_timers_restart_from_now() {
    let mut realm: Realm = Realm::new();
    let first = realm.set_timeout(Duration::from_millis(10), WorkId::new(1));
    let slow = realm.set_timeout(Duration::from_millis(20), WorkId::new(2));
    realm.set_timer_referenced(slow, false);
    let next = |realm: &mut Realm| match realm.run_next_timer() {
        Some(WorkItem::Timer { id, work }) => (id, work, realm.now()),
        _ => panic!("a timer was due"),
    };
    assert_eq!(
        next(&mut realm),
        (first, WorkId::new(1), Duration::from_millis(10))
    );
    // Refreshed at 10 ms it is due at 30, behind a timer armed for 25.
    assert!(realm.refresh_timer(slow));
    let between = realm.set_timeout(Duration::from_millis(15), WorkId::new(3));
    assert_eq!(
        next(&mut realm),
        (between, WorkId::new(3), Duration::from_millis(25))
    );
    assert!(!realm.has_referenced_timers());
    assert_eq!(
        next(&mut realm),
        (slow, WorkId::new(2), Duration::from_millis(30))
    );
    // Finished: nothing to restart, and nothing is re-armed.
    assert!(!realm.refresh_timer(slow));
    assert!(!realm.has_pending_work());

    // Cleared: the same.
    let doomed = realm.set_timeout(Duration::from_millis(5), WorkId::new(4));
    assert!(realm.refresh_timer(doomed));
    assert!(realm.clear_timeout(doomed));
    assert!(!realm.refresh_timer(doomed));
    assert!(realm.next_timer_deadline().is_none());

    // An interval restarts its current period, and keeps ticking at its period after.
    let start = realm.now();
    let tick = realm.set_interval(Duration::from_millis(10), WorkId::new(5));
    assert_eq!(next(&mut realm).2, start + Duration::from_millis(10));
    let before = realm.now();
    realm.set_timeout(Duration::from_millis(4), WorkId::new(6));
    assert_eq!(next(&mut realm).1, WorkId::new(6));
    assert!(realm.refresh_timer(tick));
    assert_eq!(
        next(&mut realm),
        (tick, WorkId::new(5), before + Duration::from_millis(4 + 10))
    );
    assert_eq!(next(&mut realm).2, before + Duration::from_millis(4 + 20));
    assert!(realm.clear_timeout(tick));
}

/// An unreferenced timer never keeps the loop open and never moves the virtual clock by
/// itself, but runs in its turn while a referenced one keeps the loop going.
#[test]
fn unreferenced_timers_do_not_keep_the_loop_alive() {
    let drive = |realm: &mut Realm| {
        let mut ran = Vec::new();
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime
            .drive(
                realm,
                |_, work| {
                    if let WorkItem::Timer { work, .. } = work {
                        ran.push(work);
                    }
                    Ok::<_, ()>(())
                },
                |_| Ok::<_, ()>(()),
            )
            .expect("realm drive succeeds");
        ran
    };

    // Alone, one is never run, though it is due, and the clock stays where it was.
    let mut realm: Realm = Realm::new();
    let lone = realm.set_timeout(Duration::from_millis(5), WorkId::new(1));
    assert!(realm.set_timer_referenced(lone, false));
    assert!(!realm.has_referenced_timers());
    assert!(drive(&mut realm).is_empty());
    assert_eq!(realm.now(), Duration::ZERO);

    // Beside a referenced timer it runs if it is due first, and not after the last one.
    let mut realm: Realm = Realm::new();
    let early = realm.set_timeout(Duration::from_millis(5), WorkId::new(1));
    realm.set_timeout(Duration::from_millis(10), WorkId::new(2));
    let late = realm.set_timeout(Duration::from_millis(50), WorkId::new(3));
    realm.set_timer_referenced(early, false);
    realm.set_timer_referenced(late, false);
    assert_eq!(drive(&mut realm), vec![WorkId::new(1), WorkId::new(2)]);
    assert_eq!(realm.now(), Duration::from_millis(10));

    // `ref` undoes it, a cleared or finished timer is no longer there to mark, and an
    // unreferenced interval stops holding the loop once the referenced work is gone.
    let mut realm: Realm = Realm::new();
    let id = realm.set_timeout(Duration::from_millis(5), WorkId::new(1));
    realm.set_timer_referenced(id, false);
    realm.set_timer_referenced(id, true);
    assert!(realm.has_referenced_timers());
    assert_eq!(drive(&mut realm), vec![WorkId::new(1)]);
    assert!(!realm.set_timer_referenced(id, false));
    let interval = realm.set_interval(Duration::from_millis(2), WorkId::new(2));
    realm.set_timer_referenced(interval, false);
    realm.set_timeout(Duration::from_millis(7), WorkId::new(3));
    assert_eq!(
        drive(&mut realm),
        vec![
            WorkId::new(2),
            WorkId::new(2),
            WorkId::new(2),
            WorkId::new(3)
        ]
    );
    let cleared = realm.set_timeout(Duration::from_millis(1), WorkId::new(4));
    realm.set_timer_referenced(cleared, false);
    assert!(realm.clear_timeout(cleared));
    assert!(!realm.has_referenced_timers());
}

#[test]
fn zero_interval_has_immediate_first_run_then_advances() {
    let mut realm: Realm = Realm::new();
    let interval = realm.set_interval(Duration::ZERO, WorkId::new(8));
    let mut times = Vec::new();
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime
        .drive(
            &mut realm,
            |realm, work| {
                if let WorkItem::Timer { id, .. } = work {
                    times.push(realm.now());
                    if times.len() == 3 {
                        assert!(realm.clear_timeout(id));
                    }
                    assert_eq!(id, interval);
                }
                Ok::<_, ()>(())
            },
            |_| Ok::<_, ()>(()),
        )
        .expect("realm drive succeeds");

    assert_eq!(
        times,
        [
            Duration::ZERO,
            Duration::from_millis(1),
            Duration::from_millis(2)
        ]
    );
}

#[test]
fn caller_payloads_are_returned_without_a_scheduler_side_map() {
    let mut realm: Realm<String, String> = Realm::new();
    realm.queue_microtask("promise-reaction".to_owned());
    realm.set_timeout(Duration::ZERO, "timer-callback".to_owned());
    let mut seen = Vec::new();
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime
        .drive(
            &mut realm,
            |_, work| {
                seen.push(match work {
                    WorkItem::Microtask(payload) => format!("micro:{payload}"),
                    WorkItem::Timer { work: payload, .. } => format!("timer:{payload}"),
                });
                Ok::<_, ()>(())
            },
            |_| Ok::<_, ()>(()),
        )
        .expect("realm drive succeeds");

    assert_eq!(seen, ["micro:promise-reaction", "timer:timer-callback"]);
}

#[test]
fn paused_time_advances_an_hour_without_wall_clock_waiting() {
    let mut realm: Realm = Realm::new();
    realm.set_timeout(Duration::from_secs(60 * 60), WorkId::new(9));
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    let mut seen = None;
    runtime
        .drive(
            &mut realm,
            |realm, work| {
                if let WorkItem::Timer { work, .. } = work {
                    assert_eq!(work, WorkId::new(9));
                    seen = Some(realm.now());
                }
                Ok::<_, ()>(())
            },
            |_| Ok::<_, ()>(()),
        )
        .expect("realm drive succeeds");

    assert_eq!(seen, Some(Duration::from_secs(60 * 60)));
}

#[test]
fn a_virtual_timer_is_not_starved_by_a_pending_host_read() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        false,
    );
    let listener = match next_host_completion(&mut runtime)
        .expect("listen completes")
        .result
        .expect("listen succeeds")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener, got {other:?}"),
    };
    let accept = runtime.start_host_op(
        AsyncHostOp::TcpAccept {
            listener: listener.id,
        },
        false,
    );
    let connect = runtime.start_host_op(
        AsyncHostOp::TcpConnect {
            host: "127.0.0.1".into(),
            port: listener.port.expect("ephemeral port assigned"),
        },
        false,
    );
    let mut server = None;
    for _ in 0..2 {
        let completion = next_host_completion(&mut runtime).expect("connection completes");
        if completion.request == accept {
            server = Some(match completion.result.expect("accept succeeds") {
                AsyncHostValue::AcceptedTcpStream(accepted) => accepted.resource,
                other => panic!("expected accepted stream, got {other:?}"),
            });
        } else {
            assert_eq!(completion.request, connect);
        }
    }
    let server = server.expect("accepted stream observed");
    let pending_read = runtime.start_host_op(
        AsyncHostOp::TcpRead {
            stream: server.id,
            max_bytes: 64,
        },
        false,
    );
    let mut realm: Realm = Realm::new();
    realm.set_timeout(Duration::from_secs(60 * 60), WorkId::new(91));

    let turn = runtime
        .next_turn(&mut realm)
        .expect("timer advances while read remains pending");
    assert!(matches!(
        turn,
        RuntimeTurn::Timer(WorkItem::Timer { work, .. }) if work == WorkId::new(91)
    ));
    assert_eq!(realm.now(), Duration::from_secs(60 * 60));
    assert!(runtime.cancel_host_op(pending_read));
}

#[test]
fn an_already_ready_host_completion_beats_a_due_timer() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    let request = runtime.start_host_op(
        AsyncHostOp::ResourceState {
            resource: ResourceId::new(999),
        },
        false,
    );
    let mut realm: Realm = Realm::new();
    realm.set_timeout(Duration::ZERO, WorkId::new(92));

    let turn = runtime
        .next_turn(&mut realm)
        .expect("ready host completion is selected");
    match turn {
        RuntimeTurn::Completion(completion) => {
            assert_eq!(completion.request, request);
            assert_eq!(
                completion
                    .result
                    .expect_err("unknown resource is rejected")
                    .code,
                "EBADF"
            );
        }
        RuntimeTurn::Timer(_) => panic!("a due timer ran before a ready completion"),
        RuntimeTurn::Failure(failure) => panic!("realm failed unexpectedly: {failure}"),
        RuntimeTurn::Protocol => panic!("no HTTP engine queued anything here"),
    }
    assert!(realm.has_pending_work());
}

#[test]
fn checkpoint_runs_after_each_microtask_drain() {
    let mut realm = Realm::new();
    let events = Rc::new(RefCell::new(Vec::new()));
    realm.queue_microtask(WorkId::new(1));
    realm.set_timeout(Duration::ZERO, WorkId::new(2));
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime
        .drive(
            &mut realm,
            |_, work| {
                if let WorkItem::Microtask(id) = work {
                    events.borrow_mut().push(format!("micro:{}", id.get()));
                } else {
                    events.borrow_mut().push("timer".to_owned());
                }
                Ok::<_, ()>(())
            },
            |_| {
                events.borrow_mut().push("checkpoint".to_owned());
                Ok::<_, ()>(())
            },
        )
        .expect("realm drive succeeds");

    assert_eq!(
        *events.borrow(),
        ["micro:1", "checkpoint", "timer", "checkpoint"]
    );
}

#[test]
fn captures_console_style_lines() {
    let mut host = CaptureHost::new();
    host.call(
        HostOp::WriteStdout,
        &[
            HostValue::String("answer".into()),
            HostValue::number(42.0),
            HostValue::Boolean(true),
            HostValue::Null,
        ],
    )
    .expect("write succeeds");
    host.call(HostOp::WriteStdout, &[HostValue::Undefined])
        .expect("write succeeds");

    assert_eq!(host.stdout(), "answer 42 true null\nundefined\n");
}

#[test]
fn empty_write_still_terminates_a_line() {
    let mut host = CaptureHost::default();
    host.call(HostOp::WriteStdout, &[]).expect("write succeeds");
    assert_eq!(host.stdout(), "\n");
}

#[test]
fn output_can_be_cleared_or_taken() {
    let mut host = CaptureHost::default();
    host.call(HostOp::WriteStdout, &[HostValue::String("one".into())])
        .expect("write succeeds");
    host.clear();
    assert!(host.stdout().is_empty());

    host.call(HostOp::WriteStdout, &[HostValue::String("two".into())])
        .expect("write succeeds");
    assert_eq!(host.into_stdout(), "two\n");
}

/// ADR 0040: state operations must not park the thread, because the drive calls them from
/// inside a task that is already running on the executor and a nested park deadlocks.
///
/// The type system carries most of this already — `RealmState` has no executor field, so it
/// cannot reach `Runtime::block_on` through `self`. This closes the remaining door, which is
/// reaching the ambient runtime through `Handle::current` or spawning onto it.
#[test]
fn realm_state_operations_never_park_the_thread() {
    let source = include_str!("../lib.rs");
    let header = "impl<H: RealmHostProvider> RealmState<H> {";
    let start = source
        .find(header)
        .expect("the RealmState operations block should exist");
    let mut depth = 0usize;
    let mut end = start;
    for (offset, character) in source[start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = start + offset;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(end > start, "the RealmState operations block should close");
    let block = &source[start..end];

    for forbidden in ["block_on", "Handle::current", "self.executor"] {
        assert!(
            !block.contains(forbidden),
            "`{forbidden}` appears in the RealmState operations block. These operations run \
                 inside a task on the executor, so parking there deadlocks the drive. Either keep \
                 the operation non-blocking or move it onto RealmRuntime, which owns the executor \
                 and is only reachable from outside a turn. See ADR 0040."
        );
    }
}

// ------------------------------------------------------------------------------------
// HTTP through the engines (ADR 0046): the host hands over a transport, the engine's task
// drives Hyper over it, and the queued adapter parks events for these tests to drain.
// A scripted run is "these bytes arrive, these bytes must leave".

fn scripted_http_runtime(
    resource: ResourceId,
    inbound: &[&[u8]],
    peer_closes: bool,
) -> (RealmRuntime<ScriptedHost>, ScriptedTransportLog) {
    scripted_runtime_with(
        resource,
        ScriptedTransport::new(inbound.iter().map(|chunk| chunk.to_vec()), peer_closes),
    )
}

fn scripted_runtime_with(
    resource: ResourceId,
    transport: ScriptedTransport,
) -> (RealmRuntime<ScriptedHost>, ScriptedTransportLog) {
    let log = transport.log();
    let host = ScriptedHost::new([]).with_transport(resource, transport);
    let runtime = RealmRuntime::with_host(host).expect("scripted runtime builds");
    (runtime, log)
}

#[test]
fn unattached_http_resource_cannot_be_closed_or_keep_the_realm_alive() {
    let resource = ResourceId::new(49);
    let (mut runtime, _log) = scripted_http_runtime(resource, &[], false);

    assert!(!runtime.http_owns(resource));
    for mode in [HttpCloseMode::Graceful, HttpCloseMode::Abort] {
        assert!(
            !runtime.http_close(resource, mode),
            "only an HTTP engine that owns the resource may close it"
        );
    }

    // A raw CloseResource fallback would be recorded by ScriptedHost as pending unexpected
    // work. Rejection must leave both transport ownership and scheduler liveness untouched.
    assert!(!runtime.has_pending_host_work());
    assert!(!runtime.has_referenced_host_work());
    assert!(!runtime.has_live_http_work());
    assert!(!runtime.http_owns(resource));
}

/// Let the runtime's real turn selector drive engine tasks and collect what they queued.
fn drain_server_events<H: RealmHostProvider>(
    runtime: &mut RealmRuntime<H>,
    resource: ResourceId,
    done: impl Fn(&[RuntimeHttpEvent]) -> bool,
) -> Vec<RuntimeHttpEvent> {
    let mut events = Vec::new();
    let mut realm: Realm = Realm::new();
    loop {
        if done(&events) {
            break;
        }
        assert!(
            runtime.next_referenced_turn(&mut realm).is_some(),
            "HTTP server became idle before its requested event: {events:?} (host={}, http={})",
            runtime.has_referenced_host_work(),
            runtime.has_live_http_work()
        );
        events.extend(runtime.take_queued_server_events(resource));
    }
    events
}

fn drain_client_events<H: RealmHostProvider>(
    runtime: &mut RealmRuntime<H>,
    resource: ResourceId,
    done: impl Fn(&[RuntimeHttpClientEvent]) -> bool,
) -> Vec<RuntimeHttpClientEvent> {
    let mut events = Vec::new();
    let mut realm: Realm = Realm::new();
    loop {
        if done(&events) {
            break;
        }
        assert!(
            runtime.next_referenced_turn(&mut realm).is_some(),
            "HTTP client became idle before its requested event: {events:?}"
        );
        events.extend(runtime.take_queued_client_events(resource));
    }
    events
}

fn flat(headers: &[(&str, &str)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (name, value) in headers {
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(value.as_bytes());
        bytes.push(0);
    }
    bytes
}

#[test]
fn scripted_provider_drains_response_before_same_turn_server_close() {
    let resource = ResourceId::new(45);
    let (mut runtime, log) = scripted_http_runtime(
        resource,
        &[b"GET /same-turn HTTP/1.1\r\nHost: kiln\r\nConnection: close\r\n\r\n"],
        false,
    );
    assert!(runtime.server_attach_queued(resource, false));
    assert!(runtime.http_owns(resource));
    let events = drain_server_events(&mut runtime, resource, |events| {
        events
            .iter()
            .any(|event| matches!(event, RuntimeHttpEvent::RequestEnd { .. }))
    });
    let (id, request, body) = request_events(&events);
    assert_eq!(request.target, "/same-turn");
    assert!(!request.keep_alive);
    assert!(body.is_empty());
    assert_eq!(runtime.requests_in_flight(), 1);

    // This is the callback's ordering: response.end() commits the slot, then server.close()
    // runs before Tokio gets another poll of the connection task.
    assert!(runtime.http_server().start_response(
        resource,
        id,
        200,
        b"",
        node_stream::DEFAULT_HIGH_WATER_MARK
    ));
    assert!(
        runtime
            .http_server()
            .end_response(resource, id, b"same-turn")
            .expect("the response ends")
            .ready
    );
    assert!(runtime.http_close(resource, HttpCloseMode::Graceful));
    assert!(
        log.written().is_empty(),
        "nothing reaches the transport inside the caller"
    );

    let events = drain_server_events(&mut runtime, resource, |events| {
        events.contains(&RuntimeHttpEvent::Closed)
    });
    assert_eq!(
        events,
        vec![RuntimeHttpEvent::Finish { id }, RuntimeHttpEvent::Closed],
        "the response finishes before the connection closes"
    );
    assert_eq!(
        String::from_utf8(log.written()).expect("HTTP is ASCII"),
        "HTTP/1.1 200 OK\r\ncontent-length: 9\r\nconnection: close\r\n\r\nsame-turn"
    );
    assert!(
        log.is_dropped(),
        "a closed connection lets its transport go"
    );
    assert!(!runtime.http_owns(resource));
    assert_eq!(runtime.requests_in_flight(), 0);
    assert!(!runtime.has_live_http_work());
    assert!(runtime.retire_http_connection(resource));
}

#[test]
fn queued_admission_wakes_once_then_idle_http_does_not_starve_timer() {
    let resource = ResourceId::new(48);
    let (mut runtime, _log) = scripted_http_runtime(resource, &[], false);
    assert!(runtime.server_attach_queued(resource, false));
    let mut realm: Realm = Realm::new();
    realm.set_timeout(Duration::ZERO, WorkId::new(480));
    assert!(matches!(
        runtime.next_referenced_turn(&mut realm),
        Some(RuntimeTurn::Protocol)
    ));
    assert!(matches!(
        runtime.next_referenced_turn(&mut realm),
        Some(RuntimeTurn::Timer(WorkItem::Timer { work, .. })) if work == WorkId::new(480)
    ));
    assert!(
        runtime.next_referenced_turn(&mut realm).is_none(),
        "an admitted but idle HTTP connection must not keep a realm alive"
    );
    assert!(runtime.shutdown());
}

#[test]
fn an_abort_publishes_closed_from_a_task_and_never_inside_the_caller() {
    let resource = ResourceId::new(46);
    let (mut runtime, log) = scripted_http_runtime(resource, &[b"GET /"], false);
    assert!(runtime.server_attach_queued(resource, false));
    let mut realm: Realm = Realm::new();
    let _ = runtime.next_referenced_turn(&mut realm);
    assert!(runtime.http_close(resource, HttpCloseMode::Abort));
    assert!(
        runtime.take_queued_server_events(resource).is_empty(),
        "the caller does not hear Closed from inside its own close"
    );
    assert!(
        runtime.has_live_http_work(),
        "the abort counts as work until the language has heard it"
    );
    let events = drain_server_events(&mut runtime, resource, |events| {
        events.contains(&RuntimeHttpEvent::Closed)
    });
    assert_eq!(events, vec![RuntimeHttpEvent::Closed]);
    assert!(log.is_dropped());
    assert!(!runtime.http_owns(resource));
    assert!(!runtime.has_live_http_work());
}

#[test]
fn realm_shutdown_aborts_engine_connections_and_rejects_new_admission() {
    let resource = ResourceId::new(47);
    let (mut runtime, log) =
        scripted_http_runtime(resource, &[b"GET / HTTP/1.1\r\nHost: kiln\r\n\r\n"], false);
    assert!(runtime.server_attach_queued(resource, false));
    assert!(
        !runtime.server_attach_queued(resource, false),
        "duplicate resource ownership is rejected"
    );
    assert!(
        runtime.has_live_http_work(),
        "a rejected duplicate attach must not erase the original admission"
    );
    assert!(
        !runtime.client_attach_queued(resource),
        "nor may the other engine take an owned resource"
    );
    let _ = drain_server_events(&mut runtime, resource, |events| {
        events
            .iter()
            .any(|event| matches!(event, RuntimeHttpEvent::RequestEnd { .. }))
    });
    assert!(runtime.has_live_http_work());

    assert!(runtime.shutdown());
    assert!(runtime.is_shutdown());
    assert!(!runtime.shutdown(), "terminal teardown is idempotent");
    assert!(!runtime.http_owns(resource));
    assert!(!runtime.has_live_http_work());
    assert!(runtime.take_queued_server_events(resource).is_empty());
    assert_eq!(
        runtime.try_start_host_op(
            AsyncHostOp::ReadFile {
                path: "must-not-start".into(),
            },
            true,
        ),
        None
    );
    let origin = HttpOrigin::http("closed.example", 80);
    assert!(!runtime.client_connection_connected(origin.clone(), resource, 1));
    assert_eq!(
        runtime.release_client_connection(resource, true),
        AgentRelease::Retire
    );
    assert_eq!(
        runtime.client_connection_failed(&origin, 1),
        AgentFailure::Empty
    );
    assert_eq!(runtime.client_connection_closed(resource), None);
    // An aborted task is dropped when its executor next runs, or goes away with it.
    drop(runtime);
    assert!(log.is_dropped(), "shutdown drops the transports it took");
}

#[test]
fn direct_engine_close_settles_pre_first_poll_admissions() {
    let server_resource = ResourceId::new(60);
    let client_resource = ResourceId::new(61);
    let host = ScriptedHost::new([])
        .with_transport(server_resource, ScriptedTransport::new([], false))
        .with_transport(client_resource, ScriptedTransport::new([], false));
    let mut runtime = RealmRuntime::with_host(host).expect("scripted runtime builds");

    assert!(runtime.server_attach_queued(server_resource, false));
    let executor = &runtime.executor;
    let server_engine = &mut runtime.state.http_server;
    assert!(executor.local.block_on(&executor.reactor.runtime, async {
        server_engine.close(server_resource, HttpCloseMode::Abort)
    }));
    let mut realm: Realm = Realm::new();
    let _ = runtime.next_referenced_turn(&mut realm);
    assert_eq!(
        runtime.take_queued_server_events(server_resource),
        vec![RuntimeHttpEvent::Closed]
    );
    assert!(runtime.next_referenced_turn(&mut realm).is_none());

    assert!(runtime.client_attach_queued(client_resource));
    let executor = &runtime.executor;
    let client_engine = &mut runtime.state.http_client;
    assert!(executor.local.block_on(&executor.reactor.runtime, async {
        client_engine.close(client_resource, HttpCloseMode::Abort)
    }));
    let events = drain_client_events(&mut runtime, client_resource, |events| {
        events.contains(&RuntimeHttpClientEvent::Closed)
    });
    assert_eq!(events, vec![RuntimeHttpClientEvent::Closed]);
    assert!(runtime.next_referenced_turn(&mut realm).is_none());
}

/// Drive one scripted HTTP/1 request through the engine and return the bytes the
/// response put on the transport. The head is what these tests are about.
fn response_head_for(
    request: &[u8],
    status: u16,
    headers: &[(&str, &str)],
    chunks: &[&[u8]],
    body: &[u8],
) -> String {
    let resource = ResourceId::new(77);
    let (mut runtime, log) = scripted_http_runtime(resource, &[request], false);
    assert!(runtime.server_attach_queued(resource, false));
    let events = drain_server_events(&mut runtime, resource, |events| {
        events
            .iter()
            .any(|event| matches!(event, RuntimeHttpEvent::RequestEnd { .. }))
    });
    let (id, _, _) = request_events(&events);
    assert!(runtime.http_server().start_response(
        resource,
        id,
        status,
        &flat(headers),
        node_stream::DEFAULT_HIGH_WATER_MARK,
    ));
    for chunk in chunks {
        assert!(
            runtime
                .http_server()
                .write_response(resource, id, chunk)
                .is_some()
        );
    }
    assert!(
        runtime
            .http_server()
            .end_response(resource, id, body)
            .is_some()
    );
    let _ = drain_server_events(&mut runtime, resource, |events| {
        events
            .iter()
            .any(|event| matches!(event, RuntimeHttpEvent::Finish { .. }))
    });
    String::from_utf8(log.written()).expect("an HTTP head is ASCII")
}

const GET: &[u8] = b"GET /framing HTTP/1.1\r\nHost: kiln\r\n\r\n";
const HEAD: &[u8] = b"HEAD /framing HTTP/1.1\r\nHost: kiln\r\n\r\n";

#[test]
fn end_with_the_whole_body_declares_its_length_in_bytes() {
    // Case 1: nothing flushed, no prior write, no framing header the script chose.
    assert_eq!(
        response_head_for(GET, 200, &[], &[], b"kiln"),
        "HTTP/1.1 200 OK\r\ncontent-length: 4\r\n\r\nkiln"
    );
    // A length is bytes, not characters: five characters, six UTF-8 bytes.
    assert_eq!(
        response_head_for(GET, 200, &[], &[], "h\u{e9}llo".as_bytes()),
        "HTTP/1.1 200 OK\r\ncontent-length: 6\r\n\r\nh\u{e9}llo"
    );
}

#[test]
fn end_with_no_body_declares_zero() {
    // Case 2.
    assert_eq!(
        response_head_for(GET, 200, &[], &[], b""),
        "HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n"
    );
}

#[test]
fn a_framing_the_script_chose_is_left_alone() {
    // Case 3, both directions. A declared length is not recomputed, and a script that asked
    // for chunking still gets it even though the whole body arrived with `end`.
    assert_eq!(
        response_head_for(GET, 200, &[("content-length", "4")], &[], b"kiln"),
        "HTTP/1.1 200 OK\r\ncontent-length: 4\r\n\r\nkiln"
    );
    assert_eq!(
        response_head_for(GET, 200, &[("transfer-encoding", "chunked")], &[], b"kiln"),
        "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n4\r\nkiln\r\n0\r\n\r\n"
    );
    // The header name is matched without regard to case, as HTTP requires.
    assert_eq!(
        response_head_for(GET, 200, &[("Content-Length", "4")], &[], b"kiln"),
        "HTTP/1.1 200 OK\r\ncontent-length: 4\r\n\r\nkiln"
    );
}

#[test]
fn a_chunk_before_end_keeps_the_body_chunked() {
    // Case 4: the length is not known when the head has to go out.
    assert_eq!(
        response_head_for(GET, 200, &[], &[b"ki"], b"ln"),
        "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n2\r\nki\r\n2\r\nln\r\n0\r\n\r\n"
    );
}

#[test]
fn a_response_that_cannot_carry_a_body_declares_no_length() {
    // Case 5. A HEAD response and a bodiless status get neither header, which is what node
    // sends; hyper forwards a content-length here rather than dropping it, so the rule has to
    // decline to add one.
    assert_eq!(
        response_head_for(HEAD, 200, &[], &[], b"kiln"),
        "HTTP/1.1 200 OK\r\n\r\n"
    );
    assert_eq!(
        response_head_for(GET, 204, &[], &[], b""),
        "HTTP/1.1 204 No Content\r\n\r\n"
    );
    assert_eq!(
        response_head_for(GET, 304, &[], &[], b""),
        "HTTP/1.1 304 Not Modified\r\n\r\n"
    );
}

#[test]
fn scripted_provider_dispatches_buffered_http1_pipeline_after_first_response() {
    let resource = ResourceId::new(44);
    let (mut runtime, log) = scripted_http_runtime(
        resource,
        &[b"GET /first HTTP/1.1\r\nHost: kiln\r\n\r\nGET /second HTTP/1.1\r\nHost: kiln\r\n\r\n"],
        false,
    );
    assert!(runtime.server_attach_queued(resource, false));
    let first_events = drain_server_events(&mut runtime, resource, |events| {
        events
            .iter()
            .any(|event| matches!(event, RuntimeHttpEvent::RequestEnd { .. }))
    });
    let (first_id, first, _) = request_events(&first_events);
    assert_eq!(first.target, "/first");
    assert!(
            !first_events
                .iter()
                .any(|event| matches!(event, RuntimeHttpEvent::RequestHead { request, .. } if request.target == "/second")),
            "the second request waits for the first response"
        );
    assert!(runtime.http_server().start_response(
        resource,
        first_id,
        200,
        b"",
        node_stream::DEFAULT_HIGH_WATER_MARK,
    ));
    assert!(
        runtime
            .http_server()
            .end_response(resource, first_id, b"/first")
            .is_some()
    );
    let second_events = drain_server_events(&mut runtime, resource, |events| {
        events.iter().any(|event| {
            matches!(
                event,
                RuntimeHttpEvent::RequestHead { request, .. } if request.target == "/second"
            )
        })
    });
    assert!(
        second_events.iter().any(|event| matches!(
            event,
            RuntimeHttpEvent::RequestHead { request, .. } if request.target == "/second"
        )),
        "buffered pipeline events: {second_events:?}"
    );
    assert!(
        log.written()
            .starts_with(b"HTTP/1.1 200 OK\r\ncontent-length: 6\r\n\r\n/first"),
        "the first response went out before the second request was dispatched"
    );
}

#[test]
fn runtime_http_response_body_preserves_node_write_and_drain_semantics() {
    use crate::node_stream::WriteDisposition;

    let resource = ResourceId::new(92);
    let (mut runtime, _log) = scripted_http_runtime(
        resource,
        &[b"GET /stream HTTP/1.1\r\nhost: kiln\r\nconnection: close\r\n\r\n"],
        false,
    );
    assert!(runtime.server_attach_queued(resource, false));
    let events = drain_server_events(&mut runtime, resource, |events| {
        events
            .iter()
            .any(|event| matches!(event, RuntimeHttpEvent::RequestEnd { .. }))
    });
    let (id, _, body) = request_events(&events);
    assert!(body.is_empty());
    assert!(runtime.http_server().start_response(
        resource,
        id,
        200,
        &flat(&[("content-type", "text/plain")]),
        8,
    ));
    assert_eq!(
        runtime.http_server().write_response(resource, id, &[1; 4]),
        Some(WriteDisposition {
            ready: true,
            submit_now: true,
        })
    );
    assert_eq!(
        runtime.http_server().write_response(resource, id, &[2; 4]),
        Some(WriteDisposition {
            ready: false,
            submit_now: false,
        })
    );
    assert_eq!(
        runtime.http_server().end_response(resource, id, &[]),
        Some(WriteDisposition {
            ready: false,
            submit_now: false,
        })
    );
    let events = drain_server_events(&mut runtime, resource, |events| {
        events.contains(&RuntimeHttpEvent::Closed)
    });
    let drain_count = events
        .iter()
        .filter(|event| **event == RuntimeHttpEvent::Drain { id })
        .count();
    let finish_count = events
        .iter()
        .filter(|event| **event == RuntimeHttpEvent::Finish { id })
        .count();
    assert_eq!(drain_count, 1, "{events:?}");
    assert_eq!(finish_count, 1, "{events:?}");
}

#[test]
fn a_scripted_client_connection_round_trips_through_the_queued_adapter() {
    let resource = ResourceId::new(51);
    // The reply is scripted, so it arrives once the request has left.
    let (mut runtime, log) = scripted_runtime_with(
            resource,
            ScriptedTransport::replying(
                [b"HTTP/1.1 201 Created\r\ncontent-type: text/plain\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok".to_vec()],
                true,
            ),
        );
    assert!(runtime.client_attach_queued(resource));
    assert!(runtime.http_client().send_request(
        resource,
        1,
        "POST",
        "/upload",
        &[("host".into(), "kiln".into())],
        node_stream::DEFAULT_HIGH_WATER_MARK,
    ));
    assert!(
        runtime
            .http_client()
            .end_request(resource, 1, b"kiln")
            .expect("the request ends")
            .ready
    );
    assert!(runtime.has_live_http_work());
    let events = drain_client_events(&mut runtime, resource, |events| {
        events.contains(&RuntimeHttpClientEvent::Closed)
    });
    let request = String::from_utf8(log.written()).expect("HTTP is ASCII");
    assert!(
        request.starts_with("POST /upload HTTP/1.1\r\n"),
        "{request}"
    );
    assert!(request.contains("content-length: 4\r\n"), "{request}");
    assert!(request.ends_with("\r\n\r\nkiln"), "{request}");
    assert_eq!(
        events,
        vec![
            RuntimeHttpClientEvent::Finish { id: 1 },
            RuntimeHttpClientEvent::ResponseHead {
                id: 1,
                status: 201,
                reason: "Created".into(),
                headers: vec![
                    ("content-type".into(), "text/plain".into()),
                    ("content-length".into(), "2".into()),
                    ("connection".into(), "close".into()),
                ],
            },
            RuntimeHttpClientEvent::ResponseData {
                id: 1,
                data: b"ok".to_vec(),
            },
            RuntimeHttpClientEvent::ResponseEnd {
                id: 1,
                reusable: false,
            },
            RuntimeHttpClientEvent::Closed,
        ]
    );
    assert!(!runtime.has_live_http_work());
    assert!(!runtime.http_owns(resource));
}

#[test]
fn a_live_tokio_stream_is_handed_to_the_engine_and_served() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        false,
    );
    let listener = match next_host_completion(&mut runtime)
        .expect("listen completes")
        .result
        .expect("listen succeeds")
    {
        AsyncHostValue::Resource(resource) => resource,
        other => panic!("expected listener, got {other:?}"),
    };
    let accept = runtime.start_host_op(
        AsyncHostOp::TcpAccept {
            listener: listener.id,
        },
        false,
    );
    let connect = runtime.start_host_op(
        AsyncHostOp::TcpConnect {
            host: "127.0.0.1".into(),
            port: listener.port.expect("listener has a port"),
        },
        false,
    );
    let mut server = None;
    let mut client = None;
    for _ in 0..2 {
        let completion = next_host_completion(&mut runtime).expect("connect completes");
        if completion.request == accept {
            server = Some(match completion.result.expect("accept succeeds") {
                AsyncHostValue::AcceptedTcpStream(stream) => stream.resource,
                other => panic!("expected accepted stream, got {other:?}"),
            });
        } else if completion.request == connect {
            client = Some(match completion.result.expect("connect succeeds") {
                AsyncHostValue::Resource(resource) => resource,
                other => panic!("expected client stream, got {other:?}"),
            });
        }
    }
    let server = server.expect("accepted stream exists");
    let client = client.expect("client stream exists");
    assert!(runtime.server_attach_queued(server.id, false));
    let waiting_accept = runtime.start_host_op(
        AsyncHostOp::TcpAccept {
            listener: listener.id,
        },
        true,
    );
    assert!(
        runtime.has_referenced_host_work(),
        "the referenced listener accept keeps the realm alive while waiting for request data"
    );
    runtime.start_host_op(
            AsyncHostOp::TcpWrite {
                stream: client.id,
                data: b"POST /health HTTP/1.1\r\nhost: kiln\r\ncontent-length: 4\r\nconnection: close\r\n\r\nping".to_vec(),
            },
            false,
        );
    next_host_completion(&mut runtime).expect("the client write completes");
    let events = drain_server_events(&mut runtime, server.id, |events| {
        events
            .iter()
            .any(|event| matches!(event, RuntimeHttpEvent::RequestEnd { .. }))
    });
    let (id, request, body) = request_events(&events);
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/health");
    assert_eq!(body, b"ping");
    assert!(runtime.cancel_host_op(waiting_accept));
    assert!(
        !runtime.has_referenced_host_work(),
        "once the server listener is unrefed, idle transports do not keep the realm alive"
    );
    assert!(runtime.http_server().start_response(
        server.id,
        id,
        200,
        &flat(&[("content-type", "text/plain")]),
        node_stream::DEFAULT_HIGH_WATER_MARK,
    ));
    assert!(
        runtime
            .http_server()
            .end_response(server.id, id, b"kiln")
            .is_some()
    );
    runtime.start_host_op(
        AsyncHostOp::TcpRead {
            stream: client.id,
            max_bytes: 4096,
        },
        false,
    );
    let response = match next_host_completion(&mut runtime)
        .expect("the client read completes")
        .result
        .expect("the read succeeds")
    {
        AsyncHostValue::Bytes(bytes) => String::from_utf8(bytes).expect("ASCII"),
        other => panic!("expected bytes, got {other:?}"),
    };
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("\r\n\r\nkiln"), "{response}");
    assert!(!response.to_ascii_lowercase().contains("\r\ndate:"));
    let events = drain_server_events(&mut runtime, server.id, |events| {
        events.contains(&RuntimeHttpEvent::Closed)
    });
    assert!(
        events.contains(&RuntimeHttpEvent::Finish { id }),
        "{events:?}"
    );
    assert!(events.contains(&RuntimeHttpEvent::Closed), "{events:?}");
    for resource in [client.id, listener.id] {
        runtime.start_host_op(AsyncHostOp::CloseResource { resource }, false);
        next_host_completion(&mut runtime).expect("close completes");
    }
}

#[test]
fn tokio_host_intercepts_vfs_overlay_operations_fast_path() {
    use kiln_host_abi::{DirEntry, FileStat, OverlayFileSystem};
    use std::collections::HashMap;
    use std::path::PathBuf;

    let mut overlay = HashMap::new();
    overlay.insert(
        PathBuf::from("/vfs/app.js"),
        b"console.log('hello vfs');".to_vec(),
    );
    let vfs = OverlayFileSystem::new(kiln_host_abi::OsFileSystem, overlay);

    let mut host = TokioHost::new().with_fs(Box::new(vfs));

    // 1. ReadFile fast path
    let req = kiln_host_abi::AsyncHostOps::start(
        &mut host,
        AsyncHostOp::ReadFile {
            path: "/vfs/app.js".to_string(),
        },
        true,
    );
    // Completed immediately into ready table!
    let comp = host.next_completion().expect("ready immediately");
    assert_eq!(comp.request, req);
    assert_eq!(
        comp.result,
        Ok(AsyncHostValue::Bytes(b"console.log('hello vfs');".to_vec()))
    );

    // 2. Stat fast path
    let req = kiln_host_abi::AsyncHostOps::start(
        &mut host,
        AsyncHostOp::Stat {
            path: "/vfs/app.js".to_string(),
        },
        true,
    );
    let comp = host.next_completion().expect("ready immediately");
    assert_eq!(comp.request, req);
    assert_eq!(
        comp.result,
        Ok(AsyncHostValue::FileStat(FileStat {
            size: 25,
            is_file: true,
            is_directory: false,
            is_symlink: false,
        }))
    );

    // 3. Lstat fast path
    let req = kiln_host_abi::AsyncHostOps::start(
        &mut host,
        AsyncHostOp::Lstat {
            path: "/vfs/app.js".to_string(),
        },
        true,
    );
    let comp = host.next_completion().expect("ready immediately");
    assert_eq!(comp.request, req);
    assert_eq!(
        comp.result,
        Ok(AsyncHostValue::FileStat(FileStat {
            size: 25,
            is_file: true,
            is_directory: false,
            is_symlink: false,
        }))
    );

    // 4. ReadDir fast path
    let req = kiln_host_abi::AsyncHostOps::start(
        &mut host,
        AsyncHostOp::ReadDir {
            path: "/vfs".to_string(),
            with_types: false,
        },
        true,
    );
    let comp = host.next_completion().expect("ready immediately");
    assert_eq!(comp.request, req);
    assert_eq!(
        comp.result,
        Ok(AsyncHostValue::DirEntries(vec![DirEntry {
            name: "app.js".to_string(),
            is_file: true,
            is_directory: false,
            is_symlink: false,
        }]))
    );
}

#[test]
fn pending_only_cancel_preserves_ready_results_and_cancels_unfinished_accept() {
    let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
    runtime.start_host_op(
        AsyncHostOp::TcpListen {
            host: "127.0.0.1".into(),
            port: 0,
        },
        true,
    );
    let listener = match next_host_completion(&mut runtime)
        .expect("listen completes")
        .result
        .expect("listen succeeds")
    {
        AsyncHostValue::Resource(resource) => resource.id,
        other => panic!("expected listener, got {other:?}"),
    };
    let query = runtime.start_host_op(AsyncHostOp::ResourceState { resource: listener }, false);
    assert_eq!(
        runtime.state.cancel_host_op_if_pending(query),
        PendingCancellation::CompletionReady
    );
    let result = next_host_completion(&mut runtime).expect("ready result remains deliverable");
    assert_eq!(result.request, query);
    assert!(
        matches!(result.result, Ok(AsyncHostValue::ResourceState(state)) if state.info.id == listener)
    );
    assert_eq!(
        runtime.state.cancel_host_op_if_pending(query),
        PendingCancellation::Unknown
    );

    // There is no peer: this accept cannot have a ready success hidden behind delivery.
    let accept = runtime.start_host_op(AsyncHostOp::TcpAccept { listener }, true);
    assert_eq!(
        runtime.state.cancel_host_op_if_pending(accept),
        PendingCancellation::Cancelled
    );
    assert_eq!(
        runtime.state.cancel_host_op_if_pending(accept),
        PendingCancellation::Unknown
    );
    runtime.start_host_op(AsyncHostOp::CloseResource { resource: listener }, false);
    let closed = next_host_completion(&mut runtime).expect("listener close completes");
    assert_eq!(closed.result, Ok(AsyncHostValue::Undefined));
    assert!(!runtime.state.has_pending_host_work());
}

#[test]
fn pending_only_cancel_preserves_synchronous_tcp_write_success() {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener binds");
    let mut runtime = RealmRuntime::new().expect("runtime builds");
    runtime.start_host_op(
        AsyncHostOp::TcpConnect {
            host: "127.0.0.1".into(),
            port: listener.local_addr().expect("listener address").port(),
        },
        true,
    );
    let stream = match next_host_completion(&mut runtime)
        .expect("connect completes")
        .result
        .expect("connect succeeds")
    {
        AsyncHostValue::Resource(resource) => resource.id,
        other => panic!("expected stream, got {other:?}"),
    };
    let (mut peer, _) = listener.accept().expect("peer accepts");
    peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .expect("bounded read");
    let write = runtime.start_host_op(
        AsyncHostOp::TcpWrite {
            stream,
            data: b"x".to_vec(),
        },
        true,
    );
    assert_eq!(
        runtime.state.cancel_host_op_if_pending(write),
        PendingCancellation::CompletionReady
    );
    let completion = next_host_completion(&mut runtime).expect("write result retained");
    assert_eq!(completion.request, write);
    assert_eq!(completion.result, Ok(AsyncHostValue::Undefined));
    let mut received = [0];
    peer.read_exact(&mut received)
        .expect("actual byte delivered");
    assert_eq!(&received, b"x");
    runtime.start_host_op(AsyncHostOp::CloseResource { resource: stream }, false);
    assert_eq!(
        next_host_completion(&mut runtime).expect("close").result,
        Ok(AsyncHostValue::Undefined)
    );
}
