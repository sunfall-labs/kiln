use super::INVALID_ARGUMENT;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;

/// An emitted chunk worker: `[start, end)` of `in_` to the same range of `out`, with `ctx`
/// handed back untouched.
pub type ChunkFn = unsafe extern "C" fn(*const c_void, *mut c_void, usize, usize, *mut c_void);

/// Below this many elements the whole range runs on the calling thread.
///
/// Measured on an 18-core M-series laptop at load average 7 (another agent benchmarking
/// alongside), `bench/non-io/parallel-map-large.mjs`'s body per element -- a square root, a
/// `pow`, a remainder and a `floor`, ~9 ns inline -- pool against inline, us per call:
///
/// | elements | pool | inline |
/// | --- | --- | --- |
/// | 2,048 | 14.9 | 14.8 |
/// | 4,096 | 97.6 | 32.1 |
/// | 8,192 | 107.8 | 63.1 |
/// | 16,384 | 125.2 | 128.2 |
/// | 65,536 | 197.4 | 543.6 |
///
/// The fork-join costs 60-90 us of wake-up and join here, so it breaks even near 16k of that
/// body and loses below; a lighter body (`x * 2 + 1`, ~1 ns) needs several times that. The
/// floor sits at the heavy body's break-even, so `multicore-parallel.mjs`'s 5,000-element
/// `x * 2 + 1` runs inline and does not regress, and a 2e6-element map is 8x faster.
pub const SEQUENTIAL_FLOOR: usize = 16384;

/// Elements per unit of work handed to the pool. Small enough that stealing evens out uneven
/// bodies across workers, large enough that the per-chunk call is noise: 1,024 elements of
/// a 15 ns body is 15 us of work against a ~50 ns call.
pub const CHUNK: usize = 1024;

/// The worker count: `KILN_PARALLELISM` when set to a positive integer, else the machine's
/// available parallelism, else 1. `1` disables the pool entirely and every call runs inline,
/// which is how a benchmark measures the sequential answer with the same binary.
fn parallelism() -> usize {
    match std::env::var("KILN_PARALLELISM") {
        Ok(value) => match value.trim().parse::<usize>() {
            Ok(n) if n >= 1 => n,
            _ => 1,
        },
        Err(_) => std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
    }
}

/// The pool, built on first use: private to this archive, so it never shares Rayon's
/// global pool with anything else linked into the process, and sized once per process.
/// `None` when the answer is one worker.
fn pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = parallelism();
        if threads <= 1 {
            return None;
        }
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("kiln-parallel-{i}"))
            .build()
            .ok()
    })
    .as_ref()
}

/// The pointers a worker needs, made `Send + Sync` for the closure Rayon runs. The C side
/// guarantees the ranges are disjoint per chunk and the arrays outlive the call.
#[derive(Clone, Copy)]
struct Job {
    in_: *const c_void,
    out: *mut c_void,
    ctx: *mut c_void,
    chunk: ChunkFn,
}
// SAFETY: each chunk writes only its own `[start, end)` of `out` and reads only its own range
// of `in_`; `ctx` is written with a relaxed atomic store by the emitted worker and read after
// the join. Nothing else on any worker touches these.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    /// One chunk. A method rather than a field read inside the closure, so the closure
    /// captures the `Sync` job as a whole and not its raw-pointer fields one by one.
    ///
    /// # Safety
    ///
    /// `[start, end)` must be within the arrays and disjoint from every other chunk in flight.
    unsafe fn run(&self, start: usize, end: usize) {
        // SAFETY: the caller's contract, which is `kiln_parallel_map`'s.
        unsafe { (self.chunk)(self.in_, self.out, start, end, self.ctx) }
    }
}

/// Run `chunk` over `[0, count)` of `in_` into `out`, split across the pool.
///
/// Returns 0 when every chunk ran, -1 for a null worker, and -2 when a worker panicked, in
/// which case some of `out` is unwritten and the caller takes its sequential path. The
/// worker signals its own failure (a typed entry that bailed out) through `ctx`, which this
/// function never reads.
///
/// # Safety
///
/// `in_` must be readable and `out` writable for `count` elements of whatever the worker
/// reads and writes, both must stay valid for the call, and `chunk` must be a function that
/// touches only its own range and thread-safe state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_parallel_map(
    in_: *const c_void,
    count: usize,
    out: *mut c_void,
    chunk: Option<ChunkFn>,
    ctx: *mut c_void,
) -> i32 {
    let Some(chunk) = chunk else {
        return INVALID_ARGUMENT;
    };
    let job = Job {
        in_,
        out,
        ctx,
        chunk,
    };
    let pool = match pool() {
        Some(pool) if count >= SEQUENTIAL_FLOOR => pool,
        _ => {
            // SAFETY: the caller contract, for the whole range on this thread.
            unsafe { job.run(0, count) };
            return 0;
        }
    };
    let chunks = count.div_ceil(CHUNK);
    let ran = catch_unwind(AssertUnwindSafe(|| {
        pool.install(|| {
            use rayon::prelude::*;
            (0..chunks).into_par_iter().for_each(|k| {
                let start = k.saturating_mul(CHUNK);
                let end = start.saturating_add(CHUNK).min(count);
                // SAFETY: `[start, end)` is this chunk's own disjoint range of a live array.
                unsafe { job.run(start, end) };
            });
        })
    }));
    match ran {
        Ok(()) => 0,
        Err(_) => -2,
    }
}

macro_rules! assert_provider_export {
        ($(($group:ident, $name:ident, $signature:ident);)*) => {
            $(assert_provider_export!($group, $name, $signature);)*
        };
        (parallel, kiln_parallel_map, parallel_map) => {
            const _: unsafe extern "C" fn(*const c_void, usize, *mut c_void, Option<ChunkFn>, *mut c_void) -> i32 = kiln_parallel_map;
        };
        ($group:ident, $name:ident, $signature:ident) => {};
    }

kiln_native_provider_contract::provider_abi_registry!(assert_provider_export);

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    unsafe extern "C" fn double_it(
        in_: *const c_void,
        out: *mut c_void,
        start: usize,
        end: usize,
        ctx: *mut c_void,
    ) {
        let src = in_ as *const f64;
        let dst = out as *mut f64;
        for i in start..end {
            // SAFETY: the test owns both arrays and the ranges are within them.
            unsafe { dst.add(i).write(src.add(i).read() * 2.0) };
        }
        // SAFETY: `ctx` is the test's counter.
        unsafe {
            (*(ctx as *const AtomicUsize)).fetch_add(end.saturating_sub(start), Ordering::Relaxed)
        };
    }

    fn run(count: usize) -> (i32, Vec<f64>, usize) {
        let input: Vec<f64> = (0..count).map(|i| i as f64).collect();
        let mut output = vec![0.0; count];
        let seen = AtomicUsize::new(0);
        // SAFETY: both vectors are live for the call and sized to `count`.
        let status = unsafe {
            kiln_parallel_map(
                input.as_ptr().cast(),
                count,
                output.as_mut_ptr().cast(),
                Some(double_it),
                &seen as *const AtomicUsize as *mut c_void,
            )
        };
        (status, output, seen.load(Ordering::Relaxed))
    }

    #[test]
    fn every_element_is_visited_exactly_once_below_and_above_the_floor() {
        for count in [0, 1, SEQUENTIAL_FLOOR - 1, SEQUENTIAL_FLOOR, 10 * CHUNK + 7] {
            let (status, out, seen) = run(count);
            assert_eq!(status, 0, "count {count}");
            assert_eq!(seen, count, "count {count}");
            assert!(out.iter().enumerate().all(|(i, &v)| v == 2.0 * i as f64));
        }
    }

    #[test]
    fn a_null_worker_is_an_invalid_argument() {
        // SAFETY: nothing is dereferenced for a null worker.
        let status = unsafe {
            kiln_parallel_map(
                std::ptr::null(),
                3,
                std::ptr::null_mut(),
                None,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(status, INVALID_ARGUMENT);
    }
}
