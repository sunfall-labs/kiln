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

/// Run a native executable with the caller's actual standard descriptors. Keeping stdout and
/// stderr attached to their original descriptors preserves kernel ordering when the caller has
/// redirected both streams to one destination.
pub fn inherit_built_executable(command: &mut Command) -> io::Result<Output> {
    let status = retry_while_executable_is_busy(|| command.status())?;
    Ok(Output {
        status,
        stdout: Vec::new(),
        stderr: Vec::new(),
    })
}

/// Which of a child's standard streams a chunk of streamed output came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildStream {
    Stdout,
    Stderr,
}

/// Run a native executable to completion, handing its output to `on_chunk` as it is written.
///
/// [`output_built_executable`] holds everything until the child exits, which for a server is
/// never: the program's first line ("listening") reaches nobody until it is killed. Here two
/// reader threads forward each read to the calling thread, which invokes `on_chunk` in arrival
/// order (stdout and stderr interleaved as they were produced). Nothing is retained: the
/// returned [`Output`] carries the exit status and empty buffers. Chunks are raw bytes and may
/// split a UTF-8 sequence.
pub fn stream_built_executable(
    command: &mut Command,
    on_chunk: &mut dyn FnMut(ChildStream, &[u8]),
) -> io::Result<Output> {
    use std::io::Read;
    use std::process::Stdio;
    use std::sync::mpsc;

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = retry_while_executable_is_busy(|| command.spawn())?;
    let (sender, receiver) = mpsc::channel::<(ChildStream, Vec<u8>)>();
    let mut readers = Vec::new();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    for (stream, pipe) in [
        (
            ChildStream::Stdout,
            stdout.map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        ),
        (
            ChildStream::Stderr,
            stderr.map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        ),
    ] {
        let Some(mut pipe) = pipe else { continue };
        let sender = sender.clone();
        readers.push(std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if sender.send((stream, buffer[..read].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        }));
    }
    drop(sender);
    while let Ok((stream, chunk)) = receiver.recv() {
        on_chunk(stream, &chunk);
    }
    for reader in readers {
        let _ = reader.join();
    }
    let status = child.wait()?;
    Ok(Output {
        status,
        stdout: Vec::new(),
        stderr: Vec::new(),
    })
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
    use super::{inherit_built_executable, retry_while_executable_is_busy};
    use std::cell::Cell;
    use std::io;
    use std::time::{Duration, Instant};

    #[cfg(unix)]
    fn output_file(label: &str) -> (std::path::PathBuf, std::fs::File) {
        use std::time::SystemTime;
        let path = std::env::temp_dir().join(format!(
            "kiln-native-inherit-{label}-{}-{}.txt",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let file = std::fs::File::create(&path).unwrap();
        (path, file)
    }

    #[cfg(unix)]
    #[test]
    fn inherited_descriptors_preserve_merged_stream_order() {
        use std::process::{Command, Stdio};

        let (path, stdout) = output_file("merged");
        let stderr = stdout.try_clone().unwrap();
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("printf 'out-1\\n'; printf 'err-1\\n' >&2; printf 'out-2\\n'; printf 'err-2\\n' >&2")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        // Both child descriptors refer to the same open file description, as with `2>&1`.
        let output = inherit_built_executable(&mut command).unwrap();
        assert!(output.status.success());
        assert!(output.stdout.is_empty() && output.stderr.is_empty());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "out-1\nerr-1\nout-2\nerr-2\n"
        );
        let _ = std::fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn inherited_descriptors_keep_stdout_and_stderr_separate() {
        use std::process::{Command, Stdio};

        let (stdout_path, stdout) = output_file("stdout");
        let (stderr_path, stderr) = output_file("stderr");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("printf 'out\\n'; printf 'err\\n' >&2")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        let output = inherit_built_executable(&mut command).unwrap();
        assert!(output.status.success());
        assert_eq!(std::fs::read_to_string(&stdout_path).unwrap(), "out\n");
        assert_eq!(std::fs::read_to_string(&stderr_path).unwrap(), "err\n");
        let _ = std::fs::remove_file(stdout_path);
        let _ = std::fs::remove_file(stderr_path);
    }

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
