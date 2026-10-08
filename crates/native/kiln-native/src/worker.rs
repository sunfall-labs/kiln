use super::*;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

const MAX_EMIT_THREADS: usize = 4;

fn emit_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = kiln_jobs::budget(MAX_EMIT_THREADS);
        kiln_jobs::pool("kiln-native-emit", threads, None)
    })
    .as_ref()
}

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

/// Start expensive translation units first without changing link order or error precedence.
/// A shared cursor assigns priorities when workers actually start: simply sorting a Rayon
/// indexed iterator would still let its split chunks begin in the middle of the cost order.
pub(crate) fn compile_largest_first<T, F>(
    sizes: &[usize],
    stage: &'static str,
    work: F,
) -> Result<Vec<T>, NativeError>
where
    F: Fn(usize) -> Result<T, NativeError> + Send + Sync,
    T: Send,
{
    let mut order: Vec<_> = (0..sizes.len()).collect();
    order.sort_unstable_by_key(|&index| (std::cmp::Reverse(sizes[index]), index));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut results = compile_indexed(sizes.len(), stage, |_| {
        let index = order[next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)];
        Ok((index, run_worker(stage, || work(index))))
    })?;
    results.sort_unstable_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

/// Emits independent module translation units concurrently without allowing a large graph to
/// occupy every machine core or hold an unbounded number of large `String`s in flight.
pub(crate) fn emit_indexed<T, F>(
    count: usize,
    stage: &'static str,
    work: F,
) -> Result<Vec<T>, NativeError>
where
    F: Fn(usize) -> Result<T, NativeError> + Send + Sync,
    T: Send,
{
    // A one-module build has no parallel work to distribute. Keep the same panic conversion and
    // index order without creating this process's four-worker emission pool.
    if count <= 1 {
        return (0..count)
            .map(|index| run_worker(stage, || work(index)))
            .collect();
    }

    let run = || {
        (0..count)
            .into_par_iter()
            .map(|index| run_worker(stage, || work(index)))
            .collect::<Vec<_>>()
            .into_iter()
            .collect()
    };
    match emit_pool() {
        Some(pool) => pool.install(run),
        None => (0..count)
            .map(|index| run_worker(stage, || work(index)))
            .collect(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_single_item_emission_stay_ordered_and_catch_panics() {
        let empty = emit_indexed::<usize, _>(0, "empty emission", |_| panic!("must not run"))
            .expect("empty emission succeeds");
        assert!(empty.is_empty());

        let single = emit_indexed(1, "single emission", |index| Ok(index + 7))
            .expect("single emission succeeds");
        assert_eq!(single, vec![7]);

        let panic = emit_indexed::<usize, _>(1, "single emission", |_| panic!("worker failure"));
        assert!(matches!(
            panic,
            Err(NativeError::WorkerPanic {
                stage: "single emission"
            })
        ));
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
        kiln_native_runtime::output_built_executable(self)
    }

    #[cfg(test)]
    fn spawn_when_launchable(&mut self) -> io::Result<std::process::Child> {
        kiln_native_runtime::spawn_built_executable(self)
    }
}

pub(crate) fn run_executable(
    command: &mut Command,
    executable: &Path,
    on_output: Option<&mut dyn FnMut(kiln_native_runtime::ChildStream, &[u8])>,
    inherit_output: bool,
) -> Result<std::process::Output, NativeError> {
    let output = if inherit_output {
        kiln_native_runtime::inherit_built_executable(command)
    } else {
        match on_output {
            Some(on_output) => kiln_native_runtime::stream_built_executable(command, on_output),
            None => command.output_when_launchable(),
        }
    };
    output.map_err(|source| NativeError::Launch {
        executable: executable.to_path_buf(),
        source,
    })
}
