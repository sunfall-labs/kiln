//! How many threads a worker pool may use, and the pool.
//!
//! Every pool in kiln is a named rayon pool sized the same way: the available CPUs, at most the
//! pool's own ceiling, and at most any positive `KILN_JOBS` or `RAYON_NUM_THREADS`. That is a
//! per-pool limit, not a process-wide CPU guarantee: a process with several pools can run more
//! threads than either variable says, as the repository's CPU budget for concurrent builds
//! (`mbx`) and test runners already assumes.

use std::sync::OnceLock;

/// The variables a caller limits every pool with, in the order they are read.
pub const LIMITS: [&str; 2] = ["KILN_JOBS", "RAYON_NUM_THREADS"];

/// The threads a pool whose own ceiling is `ceiling` may use in this process.
pub fn budget(ceiling: usize) -> usize {
    static CORES: OnceLock<usize> = OnceLock::new();
    let cores =
        *CORES.get_or_init(|| std::thread::available_parallelism().map_or(1, |cores| cores.get()));
    let requested = LIMITS.map(|name| std::env::var(name).ok());
    bounded(ceiling, cores, requested.each_ref().map(Option::as_deref))
}

/// [`budget`] of `cores` CPUs and the values of [`LIMITS`]: a value that is not a positive count
/// is ignored, and there is always at least one thread.
pub fn bounded(ceiling: usize, cores: usize, requested: [Option<&str>; 2]) -> usize {
    let mut threads = ceiling.max(1).min(cores.max(1));
    for count in requested
        .into_iter()
        .flatten()
        .filter_map(|value| value.parse::<usize>().ok())
        .filter(|count| *count > 0)
    {
        threads = threads.min(count);
    }
    threads
}

/// A pool of `threads` threads named `<name>-<index>`, each with a stack of `stack` bytes if
/// given. `None` when the operating system will not create its threads, in which case a caller
/// does its work serially rather than failing over a thread limit.
pub fn pool(name: &'static str, threads: usize, stack: Option<usize>) -> Option<rayon::ThreadPool> {
    let mut builder = rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .thread_name(move |index| format!("{name}-{index}"));
    if let Some(stack) = stack {
        builder = builder.stack_size(stack);
    }
    builder.build().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pool_takes_the_least_of_its_ceiling_the_cores_and_positive_limits() {
        assert_eq!(bounded(8, 32, [None, None]), 8);
        assert_eq!(bounded(8, 2, [Some("16"), None]), 2);
        assert_eq!(bounded(8, 32, [Some("16"), Some("1")]), 1);
        assert_eq!(bounded(8, 32, [Some("2"), Some("4")]), 2);
        assert_eq!(bounded(usize::MAX, 16, [None, Some("3")]), 3);
    }

    #[test]
    fn invalid_and_nonpositive_limits_are_ignored() {
        for value in [Some("0"), Some("-1"), Some("many"), Some("")] {
            assert_eq!(bounded(8, 32, [value, None]), 8);
        }
        assert_eq!(bounded(8, 32, [Some("9999"), None]), 8);
    }

    #[test]
    fn there_is_always_a_thread() {
        assert_eq!(bounded(8, 0, [None, None]), 1);
        assert_eq!(bounded(0, 32, [None, None]), 1);
    }

    #[test]
    fn a_pool_names_its_threads() {
        let pool = pool("kiln-jobs-test", 2, Some(1 << 20)).expect("pool");
        let name = pool.install(|| std::thread::current().name().map(str::to_owned));
        assert!(name.is_some_and(|name| name.starts_with("kiln-jobs-test-")));
    }
}
