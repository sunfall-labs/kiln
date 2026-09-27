use super::INVALID_ARGUMENT;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;

/// An emitted chunk worker: `[start, end)` of `in_` to the same range of `out`, with `ctx`
/// handed back untouched.
pub type ChunkFn = unsafe extern "C" fn(*const c_void, *mut c_void, usize, usize, *mut c_void);

// -- Interim cost model and thread budget ---------------------------------------------------
//
// Everything from here to `pool()` is the interim answer of parallel-builtins issue 04. ADR 0070
// replaces this private pool with the realm scheduler (realms issue 09): chunks become scheduled
// work, the budget becomes the scheduler's, and these constants and `pool_size` go with the pool.
// The probe in `kiln_parallel_map` is the one piece worth keeping: whatever runs the chunks still
// has to know whether a call is worth splitting and how finely.

/// The probe's first batch, run on the calling thread: a call of at most this many elements
/// runs there without reading the clock and never touches the pool.
pub const PROBE_START: usize = 128;

/// Each probe batch is this many times the last, so a light body (~1 ns) reaches a timeable
/// batch in three clock reads rather than six.
pub const PROBE_GROWTH: usize = 4;

/// The probe grows its batch until one batch takes at least this long, so the per-element
/// estimate is well above the clock's resolution (tens of ns) and the cost of reading it (~20 ns).
/// The probe as a whole spends under about 6x this inline before the pool sees the call (a body
/// that just missed the mark, then one batch `PROBE_GROWTH` times larger).
pub const PROBE_BATCH_NS: u64 = 2_000;

/// Estimated remaining work below which the call finishes on the calling thread.
///
/// A fork-join over the pool costs a wake-up of every worker and a join: 45-55 us measured on an
/// 8-core/16-thread Ryzen 7 5800H with other agents' builds running, 60-90 us on an 18-core
/// M-series laptop under load (the table this replaced). Splitting `w` of work over `t` threads
/// costs about `overhead + w / t`, which beats `w` once `w` is past the overhead; the floor is
/// about twice the overhead measured here, so a call the pool would only tie stays inline, even
/// on the slower machine. The old floor was an element count (16,384) tuned for one ~9 ns body:
/// a ~1 ns body lost at 64k elements (74 us pooled against 58 us for `map`), and a 1 us body was
/// left sequential up to 16 ms of work.
pub const PARALLEL_MIN_WORK_NS: u64 = 100_000;

/// Work per chunk handed to the pool once a call goes parallel: large enough that the per-chunk
/// dispatch (~100-200 ns in Rayon) is under 1%, small enough to steal.
pub const CHUNK_TARGET_NS: u64 = 25_000;

/// At least this many chunks per pool thread, so stealing evens out uneven bodies and a thread
/// that is descheduled (the box is shared) does not hold the join for a whole slice.
pub const CHUNKS_PER_THREAD: usize = 4;

/// How finely to split `remaining` elements of a body measured at `per_element_ns`, over
/// `threads` workers: `None` keeps the rest on the calling thread.
pub fn chunk_plan(per_element_ns: f64, remaining: usize, threads: usize) -> Option<usize> {
    if threads <= 1 || remaining < 2 {
        return None;
    }
    let per_element_ns = per_element_ns.max(0.01);
    if per_element_ns * (remaining as f64) < PARALLEL_MIN_WORK_NS as f64 {
        return None;
    }
    let by_cost = (CHUNK_TARGET_NS as f64 / per_element_ns) as usize;
    let by_balance = remaining.div_ceil(threads.saturating_mul(CHUNKS_PER_THREAD));
    Some(by_cost.min(by_balance).max(1))
}

/// The pool's size: `KILN_PARALLELISM` when set (a positive integer; anything else is 1), else
/// the machine's available parallelism divided among the `KILN_WORKER_COUNT` process workers the
/// supervisor started (`KILN_WORKERS`, which sets it in every worker before its realm runs), so
/// N workers together run at most one pool thread per core rather than N per core. Unset means
/// the only realm. `1` disables the pool and every call runs inline, which is how a benchmark
/// measures the sequential answer with the same binary.
pub fn pool_size(parallelism: Option<&str>, worker_count: Option<&str>, cores: usize) -> usize {
    if let Some(value) = parallelism {
        return match value.trim().parse::<usize>() {
            Ok(n) if n >= 1 => n,
            _ => 1,
        };
    }
    let workers = worker_count
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(1);
    cores.checked_div(workers).unwrap_or(1).max(1)
}

/// The pool size for this process, read once.
fn parallelism() -> usize {
    static THREADS: OnceLock<usize> = OnceLock::new();
    *THREADS.get_or_init(|| {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        pool_size(
            std::env::var("KILN_PARALLELISM").ok().as_deref(),
            std::env::var("KILN_WORKER_COUNT").ok().as_deref(),
            cores,
        )
    })
}

/// The pool, built the first time a call decides to use it (a program whose calls all stay
/// inline never starts its threads): private to this archive, so it never shares Rayon's
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

/// How many threads [`kiln_parallel_map`] splits a call across: the pool's size, 1 when there is
/// no pool. A call site asks before it takes its pooled lowering -- one thread cannot win
/// anything, so the sequential method's own loop runs instead (parallel issue 22) -- and a tier-3
/// site sizes its worker-arena table by it (one per pool thread, one for the calling thread).
#[unsafe(no_mangle)]
pub extern "C" fn kiln_parallel_threads() -> usize {
    parallelism()
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
    let threads = parallelism();
    if threads <= 1 {
        // SAFETY: the caller contract, for the whole range on this thread.
        unsafe { job.run(0, count) };
        return 0;
    }
    // The probe: run a growing prefix inline, one batch after another, until a batch is long
    // enough to time; then decide from what that batch cost. A call too small to be worth a
    // fork-join finishes here, having paid one clock read per batch -- none at all at or below
    // `PROBE_START` elements, since the last batch of a call is never timed. That is the "never
    // slower than `map`" half of the cost model. The estimate is the last timed batch alone, the
    // largest: it is the one most like the rest of the array (in a fresh output past the first
    // page every element carries its share of a page fault, which a small first batch misses).
    let mut done = 0usize;
    let mut batch = PROBE_START;
    let mut clock: Option<std::time::Instant> = None;
    let per_element_ns = loop {
        let end = done.saturating_add(batch).min(count);
        if end == count {
            // SAFETY: `[done, count)` is the rest of the range, on this thread.
            unsafe { job.run(done, count) };
            return 0;
        }
        let started = *clock.get_or_insert_with(std::time::Instant::now);
        // SAFETY: `[done, end)` is within the arrays, and nothing else is in flight yet.
        unsafe { job.run(done, end) };
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(started).as_nanos() as u64;
        clock = Some(now);
        let ran = end.saturating_sub(done);
        done = end;
        if elapsed >= PROBE_BATCH_NS {
            break elapsed as f64 / ran as f64;
        }
        batch = batch.saturating_mul(PROBE_GROWTH);
    };
    let remaining = count.saturating_sub(done);
    let plan = chunk_plan(per_element_ns, remaining, threads);
    let Some((chunk, pool)) = plan.and_then(|chunk| Some((chunk, pool()?))) else {
        // SAFETY: `[done, count)` is the rest of the range, on this thread.
        unsafe { job.run(done, count) };
        return 0;
    };
    let chunks = remaining.div_ceil(chunk);
    let ran = catch_unwind(AssertUnwindSafe(|| {
        pool.install(|| {
            use rayon::prelude::*;
            (0..chunks).into_par_iter().for_each(|k| {
                let start = done.saturating_add(k.saturating_mul(chunk));
                let end = start.saturating_add(chunk).min(count);
                // SAFETY: `[start, end)` is this chunk's own disjoint range of a live array, past
                // the prefix the probe already wrote.
                unsafe { job.run(start, end) };
            });
        })
    }));
    match ran {
        Ok(()) => 0,
        Err(_) => -2,
    }
}

/// `parallelSort`'s orders (ADR 0069, parallel issue 10). The emitter passes one of these only
/// where the answer is fixed by the order alone: a stable sort under a consistent comparator has
/// exactly one result, so the pool's answer is `sort`'s.
///
/// `%TypedArray%.prototype.sort` with no comparator: numeric ascending, `-0` before `+0`, `NaN`
/// last (and `NaN`s kept in their order).
pub const SORT_DEFAULT: i32 = 0;
/// A comparator proven to be `(a, b) => a - b` over numbers none of which is `NaN`: ascending,
/// with `-0` and `+0` equal (kept in their order).
pub const SORT_ASCENDING: i32 = 1;
/// `(a, b) => b - a`, likewise.
pub const SORT_DESCENDING: i32 = 2;

/// The element kind of an array's own element storage -- NaN-boxed values, each a number whose
/// bits are its `double` -- beside the `kiln_typed_kind` values 0..=8 of a typed array.
pub const SORT_KIND_VALUES: i32 = 100;

/// Below this many elements the sort runs on the calling thread: a sort of 10,000 doubles takes
/// about as long as waking the pool twice (issue 04's 45-90 us fork-join).
pub const SORT_PARALLEL_MIN: usize = 10_000;

// `%TypedArray%` default order over floats (`SORT_DEFAULT`); integers compare as integers. The
// order is `kiln_number_core::typed_order`'s, which the sequential sort runs too.
use kiln_number_core::typed_order::compare_default as default_float_order;

/// `a - b < 0`, `> 0`, else equal, for numbers that are not `NaN` -- `Infinity - Infinity` is
/// `NaN`, which the comparator's caller reads as `+0`, so equal.
fn difference_order(x: f64, y: f64) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let d = x - y;
    if d < 0.0 {
        Ordering::Less
    } else if d > 0.0 {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

/// A stable sort of `slice` by `cmp`, on the pool when it is large enough and there is one.
fn stable_sort<T: Send + Copy>(slice: &mut [T], cmp: impl Fn(&T, &T) -> std::cmp::Ordering + Sync) {
    if slice.len() >= SORT_PARALLEL_MIN
        && let Some(pool) = pool()
    {
        use rayon::slice::ParallelSliceMut;
        pool.install(|| slice.par_sort_by(|a, b| cmp(a, b)));
    } else {
        slice.sort_by(|a, b| cmp(a, b));
    }
}

/// # Safety
///
/// `data` must be valid for reads and writes of `count` elements of `T`, suitably aligned (or
/// the call declines), and nothing else may touch it during the call.
unsafe fn sort_typed<T: Send + Copy + Into<f64>>(
    data: *mut c_void,
    count: usize,
    order: i32,
) -> i32 {
    if !(data as usize).is_multiple_of(std::mem::align_of::<T>()) {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    let slice = unsafe { std::slice::from_raw_parts_mut(data.cast::<T>(), count) };
    let key = |v: &T| -> f64 { (*v).into() };
    match order {
        SORT_DEFAULT => stable_sort(slice, |a, b| default_float_order(key(a), key(b))),
        SORT_ASCENDING | SORT_DESCENDING => {
            if slice.iter().any(|v| key(v).is_nan()) {
                return 1;
            }
            if order == SORT_ASCENDING {
                stable_sort(slice, |a, b| difference_order(key(a), key(b)));
            } else {
                stable_sort(slice, |a, b| difference_order(key(b), key(a)));
            }
        }
        _ => return INVALID_ARGUMENT,
    }
    0
}

/// Sort `count` elements at `data` in place for `parallelSort`: `kind` is a `kiln_typed_kind`
/// (0..=8; the BigInt kinds are not taken) or [`SORT_KIND_VALUES`] for an array's own numeric
/// elements; `order` is [`SORT_DEFAULT`] (typed arrays only), [`SORT_ASCENDING`] or
/// [`SORT_DESCENDING`].
///
/// Returns 0 when sorted, 1 when a comparator order met a `NaN` -- nothing was written and the
/// caller runs `sort` itself, whose comparator then answers `NaN` and whose order is the
/// engine's -- and -1 for an argument it does not take (also nothing written).
///
/// # Safety
///
/// `data` must be valid for reads and writes of `count` elements of the kind, and nothing else
/// may read or write it during the call (the realm thread is blocked in it).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_parallel_sort(
    data: *mut c_void,
    count: usize,
    kind: i32,
    order: i32,
) -> i32 {
    if count < 2 {
        return 0;
    }
    if data.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY (every arm): the caller's contract, for the element type `kind` names.
    unsafe {
        match kind {
            0 => sort_typed::<i8>(data, count, order),
            1 | 2 => sort_typed::<u8>(data, count, order),
            3 => sort_typed::<i16>(data, count, order),
            4 => sort_typed::<u16>(data, count, order),
            5 => sort_typed::<i32>(data, count, order),
            6 => sort_typed::<u32>(data, count, order),
            7 => sort_typed::<f32>(data, count, order),
            8 => sort_typed::<f64>(data, count, order),
            SORT_KIND_VALUES if order != SORT_DEFAULT => {
                // A number's value bits are its double; `NaN` (the one boxed spelling) is
                // refused by the comparator orders, the only ones an array takes.
                if !(data as usize).is_multiple_of(std::mem::align_of::<u64>()) {
                    return INVALID_ARGUMENT;
                }
                let slice = std::slice::from_raw_parts_mut(data.cast::<u64>(), count);
                let key = |v: &u64| f64::from_bits(*v);
                if slice.iter().any(|v| key(v).is_nan()) {
                    return 1;
                }
                if order == SORT_ASCENDING {
                    stable_sort(slice, |a, b| difference_order(key(a), key(b)));
                } else {
                    stable_sort(slice, |a, b| difference_order(key(b), key(a)));
                }
                0
            }
            _ => INVALID_ARGUMENT,
        }
    }
}

macro_rules! assert_provider_export {
        ($(($group:ident, $name:ident, $signature:ident);)*) => {
            $(assert_provider_export!($group, $name, $signature);)*
        };
        (parallel, kiln_parallel_map, parallel_map) => {
            const _: unsafe extern "C" fn(*const c_void, usize, *mut c_void, Option<ChunkFn>, *mut c_void) -> i32 = kiln_parallel_map;
        };
        (parallel, kiln_parallel_sort, parallel_sort) => {
            const _: unsafe extern "C" fn(*mut c_void, usize, i32, i32) -> i32 = kiln_parallel_sort;
        };
        (parallel, kiln_parallel_threads, parallel_threads) => {
            const _: extern "C" fn() -> usize = kiln_parallel_threads;
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
    fn every_element_is_visited_exactly_once_inline_probed_and_pooled() {
        for count in [
            0,
            1,
            PROBE_START,
            PROBE_START + 1,
            3 * PROBE_START + 5,
            1 << 20,
            (1 << 22) + 7,
        ] {
            let (status, out, seen) = run(count);
            assert_eq!(status, 0, "count {count}");
            assert_eq!(seen, count, "count {count}");
            assert!(out.iter().enumerate().all(|(i, &v)| v == 2.0 * i as f64));
        }
    }

    #[test]
    fn small_or_cheap_work_stays_on_the_calling_thread() {
        assert_eq!(chunk_plan(1.0, 50_000, 16), None, "50 us of work");
        assert_eq!(chunk_plan(1_000.0, 90, 16), None, "90 us of work");
        assert_eq!(chunk_plan(1_000.0, 1 << 20, 1), None, "one thread");
        assert!(chunk_plan(1.0, 1 << 20, 16).is_some(), "1 ms of work");
    }

    #[test]
    fn chunks_follow_the_measured_cost_and_leave_room_to_steal() {
        // A light body: the cost target decides.
        assert_eq!(chunk_plan(1.0, 10_000_000, 16), Some(25_000));
        // A heavy body: fewer elements per chunk, never zero.
        assert_eq!(chunk_plan(10_000.0, 1_000, 16), Some(2));
        assert_eq!(chunk_plan(1_000_000.0, 1_000, 16), Some(1));
        // Just past the floor: the balance bound keeps four chunks per thread.
        assert_eq!(chunk_plan(1.0, 256_000, 16), Some(4_000));
    }

    #[test]
    fn the_pool_is_shared_among_process_workers() {
        assert_eq!(pool_size(None, None, 16), 16);
        assert_eq!(pool_size(None, Some("4"), 16), 4);
        assert_eq!(pool_size(None, Some("3"), 16), 5);
        assert_eq!(pool_size(None, Some("32"), 16), 1);
        assert_eq!(pool_size(None, Some("junk"), 16), 16);
        assert_eq!(pool_size(None, Some("0"), 16), 16);
        // `KILN_PARALLELISM` still overrides, including over a worker count.
        assert_eq!(pool_size(Some("6"), Some("4"), 16), 6);
        assert_eq!(pool_size(Some("0"), None, 16), 1);
        assert_eq!(pool_size(Some("x"), None, 16), 1);
    }

    #[test]
    fn sort_orders_match_the_sequential_comparators() {
        // Large enough for the pool, with ties the stable sort must keep in order.
        let n = SORT_PARALLEL_MIN * 3 + 17;
        let mut xs: Vec<f64> = (0..n)
            .map(|i| match i % 7 {
                0 => -0.0,
                1 => 0.0,
                2 => f64::NAN,
                3 => f64::INFINITY,
                _ => ((i * 7919) % 1000) as f64 - 500.0,
            })
            .collect();
        let mut expected = xs.clone();
        expected.sort_by(|a, b| default_float_order(*a, *b));
        // SAFETY: the vector is live and sized to `n`.
        assert_eq!(
            unsafe { kiln_parallel_sort(xs.as_mut_ptr().cast(), n, 8, SORT_DEFAULT) },
            0
        );
        assert!(
            xs.iter()
                .zip(&expected)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        // A comparator order declines a NaN and writes nothing.
        let before = xs.clone();
        // SAFETY: as above.
        assert_eq!(
            unsafe { kiln_parallel_sort(xs.as_mut_ptr().cast(), n, 8, SORT_ASCENDING) },
            1
        );
        assert!(
            xs.iter()
                .zip(&before)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        // `a - b` keeps -0 and +0 in their order; `b - a` reverses the values only.
        let mut zs: Vec<u64> = (0..n)
            .map(|i| {
                if i % 2 == 0 {
                    (-0.0f64).to_bits()
                } else {
                    ((i % 5) as f64).to_bits()
                }
            })
            .collect();
        let mut expected: Vec<u64> = zs.clone();
        expected.sort_by(|a, b| difference_order(f64::from_bits(*b), f64::from_bits(*a)));
        // SAFETY: as above.
        assert_eq!(
            unsafe {
                kiln_parallel_sort(zs.as_mut_ptr().cast(), n, SORT_KIND_VALUES, SORT_DESCENDING)
            },
            0
        );
        assert_eq!(zs, expected);
        let mut ints: Vec<i32> = (0..n as i32).map(|i| (i * 7919) % 1013 - 500).collect();
        let mut sorted = ints.clone();
        sorted.sort();
        // SAFETY: as above.
        assert_eq!(
            unsafe { kiln_parallel_sort(ints.as_mut_ptr().cast(), n, 5, SORT_DEFAULT) },
            0
        );
        assert_eq!(ints, sorted);
        // An array never takes the default order (it compares strings), nor a BigInt kind.
        // SAFETY: nothing is written for an argument the call does not take.
        assert_eq!(
            unsafe {
                kiln_parallel_sort(ints.as_mut_ptr().cast(), 4, SORT_KIND_VALUES, SORT_DEFAULT)
            },
            INVALID_ARGUMENT
        );
        // SAFETY: as above.
        assert_eq!(
            unsafe { kiln_parallel_sort(ints.as_mut_ptr().cast(), 4, 9, SORT_DEFAULT) },
            INVALID_ARGUMENT
        );
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
