use super::*;
use rayon::prelude::*;
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
    const ATTEMPTS: u32 = 12;
    let mut backoff = Duration::from_millis(2);
    for attempt in 1..=ATTEMPTS {
        match operation() {
            Err(source)
                if source.kind() == io::ErrorKind::ExecutableFileBusy && attempt < ATTEMPTS =>
            {
                std::thread::sleep(backoff);
                backoff = backoff.saturating_mul(2).min(Duration::from_millis(100));
            }
            other => return other,
        }
    }
    unreachable!("the final attempt returns rather than looping")
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
