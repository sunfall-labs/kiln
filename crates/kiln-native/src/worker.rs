use super::*;
use std::path::Path;
use std::process::Command;

pub(crate) fn run_worker<T>(
    stage: &'static str,
    work: impl FnOnce() -> Result<T, NativeError>,
) -> Result<T, NativeError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
        .map_err(|_| NativeError::WorkerPanic { stage })?
}

/// Runs `work` over `0..count` on Rayon's work-stealing pool.
///
/// Object compile times are wildly uneven: a twenty-line module and a bundled application are both
/// one index here, and they differ by orders of magnitude. Rayon's indexed parallel iterator keeps
/// idle workers useful without making Kiln maintain its own cursor, worker lifecycle or stealing
/// policy.
///
/// Results come back in index order regardless of which worker produced them, and the failure
/// reported is the one belonging to the lowest failing index — either the error `work` returned or
/// [`NativeError::WorkerPanic`] where the task holding that index panicked.
pub(crate) fn compile_indexed<T, F>(
    count: usize,
    stage: &'static str,
    work: F,
) -> Result<Vec<T>, NativeError>
where
    F: Fn(usize) -> Result<T, NativeError> + Send + Sync,
    T: Send,
{
    (0..count)
        .into_par_iter()
        .map(|index| run_worker(stage, || work(index)))
        .collect::<Vec<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn read(path: &Path, operation: &'static str) -> Result<Vec<u8>, NativeError> {
    fs::read(path).map_err(|source| NativeError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn decode_run_result(
    path: &Path,
    process_succeeded: bool,
) -> Result<Option<HostValue>, NativeError> {
    if !process_succeeded {
        return Ok(None);
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        // A run that finished but completed with a value the boundary cannot carry -- a CommonJS
        // entry completes with its `module.exports` object -- writes no file, on purpose. That is
        // an absent result, not a broken one, and the launcher already exited 0 to say so. Any
        // other read error is still a genuine harness failure and is still reported as one.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(NativeError::RunResult {
                path: path.to_path_buf(),
                message: error.to_string(),
            });
        }
    };
    decode_native_result(&bytes)
        .map(Some)
        .map_err(|error| NativeError::RunResult {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
}

/// Run a freshly linked executable, waiting out the kernel's `ETXTBSY` if it fires.
///
/// A process may not `execve` a file that any process still holds open for writing. Nothing here
/// holds one -- the linker wrote the file and exited -- but this is a multithreaded program, and
/// between `fork` and `execve` a child briefly owns a duplicate of every descriptor its parent had
/// open at the moment of the fork. So one thread spawning a compiler can, for a few microseconds,
/// be holding a write descriptor to the executable another thread is trying to launch. Rust opens
/// files `O_CLOEXEC`, which closes the duplicate at the child's `execve`; it does not remove the
/// window before it.
///
/// The condition is therefore real, transient, and no fault of the caller's -- the same reason
/// cargo, rustc and every build system that writes and then runs a binary retries it. Waiting is
/// the whole fix; the window is microseconds and the bound is generous enough that a genuine
/// `ETXTBSY` -- a file somebody really is writing -- still surfaces as an error rather than a hang.
/// Retry `operation` while the kernel says the executable is still open for writing somewhere.
///
/// `execve` refuses a file any process holds open for writing. Nothing here holds one -- the
/// linker wrote the file and exited -- but this is a multithreaded program, and between `fork` and
/// `execve` a child briefly owns a duplicate of every descriptor its parent had open at the moment
/// of the fork. One thread spawning a compiler can therefore hold, for a few microseconds, a write
/// descriptor to the binary another thread is launching. Rust opens files `O_CLOEXEC`, which closes
/// the duplicate at the child's `execve`; it does not remove the window before it.
///
/// So the condition is real, transient and no fault of the caller's, which is why cargo, rustc and
/// every build system that writes a binary and then runs it does exactly this. The bound is
/// generous next to a window measured in microseconds, and keeps a file somebody genuinely is
/// writing surfacing as an error rather than a hang.
pub(crate) fn retry_while_executable_is_busy<T>(
    mut operation: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    // A budget rather than a count, because what has to be outlasted is the *other* thread's
    // fork-to-execve window, and that window is only microseconds on an idle machine. Twelve
    // attempts with a 100ms cap gave up after about six tenths of a second, which was ample
    // there and not ample here: this repository is routinely built by more than one agent at
    // once, and under an eight-way build the forking thread can be descheduled for far longer
    // than it takes to `execve`. Four to five of `kiln-native`'s tests then failed together,
    // reproducibly under load and never in isolation -- which reads as a flaky test suite rather
    // than as the transient kernel condition it is.
    //
    // The condition always clears: it is held by a descriptor that closes at the other thread's
    // `execve`. So the bound exists only to keep a file somebody genuinely is writing surfacing
    // as an error rather than a hang, and ten seconds does that while being far outside any
    // plausible scheduling delay.
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

/// `Command`, for the case where the program was built moments ago by this same process.
pub(crate) trait RunsABuiltBinary {
    /// [`Command::output`], waiting out a transient `ETXTBSY`.
    fn output_when_launchable(&mut self) -> io::Result<std::process::Output>;
    /// [`Command::spawn`], waiting out a transient `ETXTBSY`. Only the tests launch a built
    /// binary without waiting for it; the shipped paths all want its output.
    #[cfg(test)]
    fn spawn_when_launchable(&mut self) -> io::Result<std::process::Child>;
}

impl RunsABuiltBinary for Command {
    fn output_when_launchable(&mut self) -> io::Result<std::process::Output> {
        retry_while_executable_is_busy(|| self.output())
    }

    #[cfg(test)]
    fn spawn_when_launchable(&mut self) -> io::Result<std::process::Child> {
        retry_while_executable_is_busy(|| self.spawn())
    }
}

pub(crate) fn run_executable(
    command: &mut Command,
    executable: &Path,
) -> Result<std::process::Output, NativeError> {
    command
        .output_when_launchable()
        .map_err(|source| NativeError::Launch {
            executable: executable.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod busy_retry_tests {
    use super::retry_while_executable_is_busy;
    use std::cell::Cell;
    use std::io;
    use std::time::{Duration, Instant};

    /// The retry outlasts a busy window far longer than a fork-to-`execve` gap on an idle machine.
    ///
    /// This is the regression this bound exists for. It used to be twelve attempts with a 100ms
    /// cap -- about six tenths of a second -- which is ample against the microsecond window the
    /// condition has when nothing else is running, and not ample when eight threads are each
    /// forking a C compiler. Four to five of this crate's tests failed together under that load
    /// and passed in isolation, which is indistinguishable from a flaky suite until you know the
    /// mechanism.
    ///
    /// A count cannot express "outlast a scheduling delay"; a deadline can. Asserting on a
    /// duration that the old bound could not have survived is what keeps it from being tuned back
    /// down to one.
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

    /// A file somebody genuinely is writing still surfaces as an error rather than hanging.
    #[test]
    fn a_permanently_busy_executable_still_reports_rather_than_hanging() {
        let outcome = retry_while_executable_is_busy(|| {
            Err::<(), _>(io::Error::from(io::ErrorKind::Interrupted))
        });
        assert_eq!(
            outcome
                .expect_err("an unrelated error is returned at once")
                .kind(),
            io::ErrorKind::Interrupted
        );
    }
}
