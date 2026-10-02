use std::io;
use std::process::{Child, Command, Output};
use std::time::{Duration, Instant};

/// Spawn a native executable that may still be open for writing by a concurrent build process.
///
/// Linux reports that short publication window as `ETXTBSY`. This uses the same bounded retry
/// policy as [`output_built_executable`] so watched and captured executions share one authority.
pub fn spawn_built_executable(command: &mut Command) -> io::Result<Child> {
    retry_while_executable_is_busy(|| command.spawn())
}

/// Capture the output of a native executable, waiting out a transient `ETXTBSY`.
pub fn output_built_executable(command: &mut Command) -> io::Result<Output> {
    retry_while_executable_is_busy(|| command.output())
}

/// Retry an operation only while the kernel reports that its executable is still open for write.
/// The window comes from another thread's fork-to-exec interval: `O_CLOEXEC` closes inherited file
/// descriptors at exec, but not during the interval before exec begins.
fn retry_while_executable_is_busy<T>(
    mut operation: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    const BUDGET: Duration = Duration::from_secs(10);
    let deadline = Instant::now() + BUDGET;
    let mut backoff = Duration::from_millis(2);
    loop {
        match operation() {
            Err(source)
                if source.kind() == io::ErrorKind::ExecutableFileBusy
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(backoff);
                backoff = backoff.saturating_mul(2).min(Duration::from_millis(100));
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::retry_while_executable_is_busy;
    use std::cell::Cell;
    use std::io;
    use std::time::{Duration, Instant};

    #[test]
    fn a_busy_executable_is_waited_out_past_the_old_attempt_bound() {
        let started = Instant::now();
        let busy_until = started + Duration::from_millis(1_500);
        let calls = Cell::new(0u32);
        let outcome = retry_while_executable_is_busy(|| {
            calls.set(calls.get() + 1);
            if Instant::now() < busy_until {
                return Err(io::Error::from(io::ErrorKind::ExecutableFileBusy));
            }
            Ok(7u32)
        });
        assert_eq!(
            outcome.expect("the window closes, so the operation succeeds"),
            7
        );
        assert!(
            started.elapsed() >= Duration::from_millis(1_500),
            "returned before the busy window closed"
        );
        assert!(
            calls.get() > 12,
            "gave up within the old twelve-attempt bound"
        );
    }

    #[test]
    fn a_permanently_busy_executable_returns_at_the_deadline() {
        const BUDGET: Duration = Duration::from_secs(10);
        let started = Instant::now();
        let error = retry_while_executable_is_busy(|| {
            Err::<(), _>(io::Error::from(io::ErrorKind::ExecutableFileBusy))
        })
        .expect_err("a permanently busy executable must eventually return");
        assert_eq!(error.kind(), io::ErrorKind::ExecutableFileBusy);
        assert!(
            started.elapsed() >= BUDGET,
            "returned before the retry deadline"
        );
        assert!(
            started.elapsed() < BUDGET + Duration::from_secs(10),
            "retry exceeded its bounded deadline by too much"
        );
    }

    #[test]
    fn an_unrelated_spawn_error_is_returned_without_retrying() {
        let calls = Cell::new(0);
        let outcome = retry_while_executable_is_busy(|| {
            calls.set(calls.get() + 1);
            Err::<(), _>(io::Error::from(io::ErrorKind::Interrupted))
        });
        assert_eq!(
            outcome
                .expect_err("an unrelated error is returned at once")
                .kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(calls.get(), 1, "unrelated errors must not be retried");
    }
}
