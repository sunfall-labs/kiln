use crate::CaptureHost;
use kiln_host_abi::HostOps;

#[cfg(unix)]
#[test]
fn synchronous_process_authority_captures_output_and_status() {
    let mut denied = CaptureHost::new();
    assert!(
        denied
            .run_process_sync("sh", &["-c".into(), "exit 0".into()])
            .is_err()
    );

    let mut host = CaptureHost::new().with_sync_process(true);
    let output = host
        .run_process_sync(
            "sh",
            &["-c".into(), "printf out; printf err >&2; exit 7".into()],
        )
        .expect("authorized synchronous process");
    assert_eq!(output.stdout, b"out");
    assert_eq!(output.stderr, b"err");
    assert_eq!(output.status, Some(7));
    assert!(output.pid.is_some());
    assert_eq!(output.error, None);
    assert_eq!(output.signal, None);
}

#[cfg(unix)]
#[test]
fn synchronous_process_authority_reports_signal_termination() {
    let mut host = CaptureHost::new().with_sync_process(true);
    let output = host
        .run_process_sync("sh", &["-c".into(), "kill -TERM $$".into()])
        .expect("authorized synchronous process");
    assert_eq!(output.status, None);
    assert_eq!(output.signal.as_deref(), Some("SIGTERM"));
}
