use super::INVALID_ARGUMENT;
use kiln_runtime::turn_queue::TurnOutcome;
use kiln_runtime::turn_scheduler::{PinnedTurn, PinnedTurnScheduler};
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::task::Waker;

/// An emitted chunk worker: `[start, end)` of `in_` to the same range of `out`, with `ctx`
/// handed back untouched.
pub type ChunkFn = unsafe extern "C" fn(*const c_void, *mut c_void, usize, usize, *mut c_void);

// -- Interim cost model and thread budget ---------------------------------------------------
//
// The chunk cost probe remains from parallel-builtins issue 04. Map chunks and stable sort
// passes use the same realm scheduler queue (realms issue 09).

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

/// Target work per chunk once a call goes parallel. This is the pre-scheduler tuning point;
/// the scheduler benchmark will establish whether its ticket cost calls for a different value.
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

/// The process fallback for parallel calls made outside a scheduled realm. In a scheduled
/// realm, the caller borrows that realm's lane pool instead, so the call starts no extra worker
/// threads. This pool is lazy for the same reason the older Rayon pool was.
fn scheduler_pool() -> Option<Arc<PinnedTurnScheduler>> {
    static POOL: OnceLock<Option<Arc<PinnedTurnScheduler>>> = OnceLock::new();
    POOL.get_or_init(|| {
        (parallelism() > 1)
            .then(|| PinnedTurnScheduler::new(parallelism()).ok().map(Arc::new))
            .flatten()
    })
    .clone()
}

/// The pointers a worker needs, made `Send + Sync` for scheduler turns. The C side
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

struct ChunkGroup {
    next: AtomicUsize,
    running_bodies: AtomicUsize,
    failed: AtomicBool,
    chunks: usize,
    start: usize,
    chunk_size: usize,
    count: usize,
    done: Mutex<()>,
    changed: Condvar,
}

impl ChunkGroup {
    fn new(start: usize, chunk_size: usize, count: usize) -> Self {
        Self {
            next: AtomicUsize::new(0),
            running_bodies: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
            chunks: count.saturating_sub(start).div_ceil(chunk_size),
            start,
            chunk_size,
            count,
            done: Mutex::new(()),
            changed: Condvar::new(),
        }
    }

    fn run_one(&self, run: impl FnOnce(usize, usize)) -> bool {
        if self.failed.load(Ordering::Acquire) {
            return false;
        }
        let index = self.next.fetch_add(1, Ordering::AcqRel);
        if index >= self.chunks {
            return false;
        }
        let start = self
            .start
            .saturating_add(index.saturating_mul(self.chunk_size));
        let end = start.saturating_add(self.chunk_size).min(self.count);
        if catch_unwind(AssertUnwindSafe(|| run(start, end))).is_err() {
            self.failed.store(true, Ordering::Release);
            return false;
        }
        true
    }

    fn join(&self) {
        let mut guard = self.done.lock().expect("parallel join lock poisoned");
        while self.running_bodies.load(Ordering::Acquire) != 0 {
            guard = self
                .changed
                .wait(guard)
                .expect("parallel join lock poisoned");
        }
    }

    fn body_finished(&self) {
        if self.running_bodies.fetch_sub(1, Ordering::AcqRel) == 1 {
            // A notification cannot race between the joiner's check and Condvar::wait.
            let _guard = self.done.lock().expect("parallel join lock poisoned");
            self.changed.notify_all();
        }
    }
}

/// One scheduler ticket performs one chunk per turn, then yields its lane. A concurrent caller
/// always claims chunks too, so calls made on every lane cannot deadlock waiting for free lanes.
struct ChunkTurn {
    group: Arc<ChunkGroup>,
    job: Job,
}

impl ChunkTurn {
    fn new(group: Arc<ChunkGroup>, job: Job) -> Self {
        group.running_bodies.fetch_add(1, Ordering::AcqRel);
        Self { group, job }
    }
}

impl PinnedTurn for ChunkTurn {
    fn run_turn(&mut self, _waker: &Waker) -> TurnOutcome {
        if self
            .group
            .run_one(|start, end| unsafe { self.job.run(start, end) })
        {
            TurnOutcome::Ready
        } else {
            TurnOutcome::Done
        }
    }
}

impl Drop for ChunkTurn {
    fn drop(&mut self) {
        self.group.body_finished();
    }
}

/// How many threads [`kiln_parallel_map`] splits a call across: the pool's size, 1 when there is
/// no pool. A call site asks before it takes its pooled lowering -- one thread cannot win
/// anything, so the sequential method's own loop runs instead (parallel issue 22) -- and a tier-3
/// site sizes its worker-arena table by it (one per pool thread, one for the calling thread).
#[unsafe(no_mangle)]
pub extern "C" fn kiln_parallel_threads() -> usize {
    super::realm::current_scheduler()
        .map(|scheduler| scheduler.lane_count().min(parallelism()))
        .unwrap_or_else(parallelism)
}

/// Run `chunk` over `[0, count)` of `in_` into `out`, split across scheduler lanes.
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
    let threads = kiln_parallel_threads();
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
    let Some(chunk) = plan else {
        // SAFETY: `[done, count)` is the rest of the range, on this thread.
        unsafe { job.run(done, count) };
        return 0;
    };
    let Some(scheduler) = super::realm::current_scheduler().or_else(scheduler_pool) else {
        unsafe { job.run(done, count) };
        return 0;
    };
    let group = Arc::new(ChunkGroup::new(done, chunk, count));
    let worker_count = threads
        .saturating_sub(1)
        .min(group.chunks.saturating_sub(1));
    let handles = (0..worker_count)
        .map(|_| scheduler.spawn_movable(ChunkTurn::new(Arc::clone(&group), job)))
        .collect::<Vec<_>>();
    while group.run_one(|start, end| unsafe { job.run(start, end) }) {}
    for handle in &handles {
        handle.cancel();
    }
    // A racing turn may put its body back into an entry just as cancellation removes that entry
    // from the scheduler registry. The handles themselves still own those entries; release them
    // before waiting for every body destructor to report completion.
    drop(handles);
    group.join();
    if group.failed.load(Ordering::Acquire) {
        -2
    } else {
        0
    }
}

/// `parallelSort`'s orders (ADR 0069, parallel issue 10). The emitter passes one of these only
/// where the answer is fixed by the order alone: a stable sort under a consistent comparator has
/// exactly one result, so the scheduler's answer is `sort`'s.
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
/// bits are its `double` -- beside the native `kiln_typed_kind` discriminants of a typed array.
pub const SORT_KIND_VALUES: i32 = 100;

/// Below this many elements the sort runs on the calling thread: a sort of 10,000 doubles takes
/// about as long as several scheduler wake-and-join rounds.
pub const SORT_PARALLEL_MIN: usize = 10_000;

// `%TypedArray%` default order over floats (`SORT_DEFAULT`); integers compare as integers. The
// order is `kiln_number_core::typed_order`'s, which the sequential sort runs too.
use kiln_number_core::typed_order::{ElementKind, compare_default as default_float_order};

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

#[derive(Clone, Copy)]
enum SortPhase {
    Runs,
    Merge,
}

/// A phase borrows its comparator and buffers only until every scheduled body has been dropped.
struct SortContext<T, F> {
    source: *mut T,
    target: *mut T,
    len: usize,
    width: usize,
    cmp: *const F,
    phase: SortPhase,
}

#[derive(Clone, Copy)]
struct SortJob {
    context: *const c_void,
    run: unsafe fn(*const c_void, usize),
}

// SAFETY: the phase join keeps its stack context and both buffers alive. Tasks own disjoint
// runs or merge destinations; all tasks only read the shared `Sync` comparator.
unsafe impl Send for SortJob {}
unsafe impl Sync for SortJob {}

impl SortJob {
    unsafe fn run_one(self, index: usize) {
        // SAFETY: `run_sort_phase` joins all bodies before the context leaves scope.
        unsafe { (self.run)(self.context, index) }
    }
}

unsafe fn run_sort_one<T: Send + Copy, F: Fn(&T, &T) -> std::cmp::Ordering + Sync>(
    context: *const c_void,
    index: usize,
) {
    // SAFETY: the job is made only with a live SortContext<T, F> for this phase.
    let context = unsafe { &*context.cast::<SortContext<T, F>>() };
    let cmp = unsafe { &*context.cmp };
    match context.phase {
        SortPhase::Runs => {
            let start = index * context.width;
            let end = start.saturating_add(context.width).min(context.len);
            // SAFETY: each index names a disjoint mutable run within source.
            let run =
                unsafe { std::slice::from_raw_parts_mut(context.source.add(start), end - start) };
            run.sort_by(cmp);
        }
        SortPhase::Merge => {
            let start = index.saturating_mul(context.width.saturating_mul(2));
            let middle = start.saturating_add(context.width).min(context.len);
            let end = middle.saturating_add(context.width).min(context.len);
            let mut left = start;
            let mut right = middle;
            for out in start..end {
                // Equal values come from the left run first, preserving input order.
                let take_left = left < middle
                    && (right == end
                        || cmp(unsafe { &*context.source.add(left) }, unsafe {
                            &*context.source.add(right)
                        }) != std::cmp::Ordering::Greater);
                // SAFETY: source and target are separate live buffers. Each task writes only
                // its own merge interval, while all tasks read completed source runs.
                unsafe {
                    if take_left {
                        context.target.add(out).write(*context.source.add(left));
                    } else {
                        context.target.add(out).write(*context.source.add(right));
                    }
                }
                if take_left {
                    left += 1;
                } else {
                    right += 1;
                }
            }
        }
    }
}

struct SortTurn {
    group: Arc<ChunkGroup>,
    job: SortJob,
}

impl SortTurn {
    fn new(group: Arc<ChunkGroup>, job: SortJob) -> Self {
        group.running_bodies.fetch_add(1, Ordering::AcqRel);
        Self { group, job }
    }
}

impl PinnedTurn for SortTurn {
    fn run_turn(&mut self, _waker: &Waker) -> TurnOutcome {
        if self
            .group
            .run_one(|index, _| unsafe { self.job.run_one(index) })
        {
            TurnOutcome::Ready
        } else {
            TurnOutcome::Done
        }
    }
}

impl Drop for SortTurn {
    fn drop(&mut self) {
        self.group.body_finished();
    }
}

fn run_sort_phase<T: Send + Copy, F: Fn(&T, &T) -> std::cmp::Ordering + Sync>(
    source: *mut T,
    target: *mut T,
    len: usize,
    width: usize,
    phase: SortPhase,
    cmp: &F,
    scheduler: &PinnedTurnScheduler,
) {
    let tasks = match phase {
        SortPhase::Runs => len.div_ceil(width),
        SortPhase::Merge => len.div_ceil(width.saturating_mul(2)),
    };
    let context = SortContext {
        source,
        target,
        len,
        width,
        cmp,
        phase,
    };
    let job = SortJob {
        context: (&context as *const SortContext<T, F>).cast(),
        run: run_sort_one::<T, F>,
    };
    let group = Arc::new(ChunkGroup::new(0, 1, tasks));
    let workers = scheduler.lane_count().saturating_sub(1).min(tasks - 1);
    let handles = (0..workers)
        .map(|_| scheduler.spawn_movable(SortTurn::new(Arc::clone(&group), job)))
        .collect::<Vec<_>>();
    while group.run_one(|index, _| unsafe { job.run_one(index) }) {}
    for handle in &handles {
        handle.cancel();
    }
    drop(handles);
    group.join();
    assert!(
        !group.failed.load(Ordering::Acquire),
        "parallel sort task panicked"
    );
}

/// Stable run sorting followed by stable pairwise merges, on the same lanes as realm turns.
fn stable_sort<T: Send + Copy>(slice: &mut [T], cmp: impl Fn(&T, &T) -> std::cmp::Ordering + Sync) {
    let scheduler = if slice.len() >= SORT_PARALLEL_MIN && kiln_parallel_threads() > 1 {
        super::realm::current_scheduler().or_else(scheduler_pool)
    } else {
        None
    };
    let Some(scheduler) = scheduler.filter(|pool| pool.lane_count() > 1) else {
        slice.sort_by(cmp);
        return;
    };
    stable_sort_on_scheduler(slice, cmp, &scheduler);
}

fn stable_sort_on_scheduler<T: Send + Copy>(
    slice: &mut [T],
    cmp: impl Fn(&T, &T) -> std::cmp::Ordering + Sync,
    scheduler: &PinnedTurnScheduler,
) {
    let len = slice.len();
    let mut width = len.div_ceil(scheduler.lane_count().saturating_mul(CHUNKS_PER_THREAD));
    width = width.max(1_024);
    let mut scratch = slice.to_vec();
    let original = slice.as_mut_ptr();
    let temporary = scratch.as_mut_ptr();
    run_sort_phase(
        original,
        temporary,
        len,
        width,
        SortPhase::Runs,
        &cmp,
        scheduler,
    );
    let mut source = original;
    let mut target = temporary;
    while width < len {
        run_sort_phase(
            source,
            target,
            len,
            width,
            SortPhase::Merge,
            &cmp,
            scheduler,
        );
        std::mem::swap(&mut source, &mut target);
        width = width.saturating_mul(2);
    }
    if source != original {
        // SAFETY: source and original are separate buffers of `len` initialized elements.
        unsafe { std::ptr::copy_nonoverlapping(source, original, len) };
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

/// Sort `count` elements at `data` in place for `parallelSort`: `kind` is a native
/// `kiln_typed_kind` discriminant (Float16 and BigInt kinds are not taken) or
/// [`SORT_KIND_VALUES`] for an array's own numeric elements; `order` is [`SORT_DEFAULT`]
/// (typed arrays only), [`SORT_ASCENDING`] or
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
            SORT_KIND_VALUES => INVALID_ARGUMENT,
            _ => match ElementKind::from_native(kind) {
                Some(ElementKind::Int8) => sort_typed::<i8>(data, count, order),
                Some(ElementKind::Uint8 | ElementKind::Uint8Clamped) => {
                    sort_typed::<u8>(data, count, order)
                }
                Some(ElementKind::Int16) => sort_typed::<i16>(data, count, order),
                Some(ElementKind::Uint16) => sort_typed::<u16>(data, count, order),
                Some(ElementKind::Int32) => sort_typed::<i32>(data, count, order),
                Some(ElementKind::Uint32) => sort_typed::<u32>(data, count, order),
                // Float16 has no native scalar representation in this parallel helper; let
                // the sequential typed-array sort handle it. Float32/64 and BigInt therefore
                // follow the same discriminant authority as the sequential provider entry.
                Some(ElementKind::Float32) => sort_typed::<f32>(data, count, order),
                Some(ElementKind::Float64) => sort_typed::<f64>(data, count, order),
                Some(ElementKind::Float16 | ElementKind::BigInt64 | ElementKind::BigUint64)
                | None => INVALID_ARGUMENT,
            },
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
    fn scheduler_merge_preserves_ties_across_run_boundaries() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        struct Item {
            key: usize,
            original: usize,
        }

        let scheduler = PinnedTurnScheduler::new(2).expect("sort lanes start");
        let mut items = (0..30_017)
            .map(|original| Item {
                key: (original * 7919) % 17,
                original,
            })
            .collect::<Vec<_>>();
        let mut expected = items.clone();
        expected.sort_by_key(|item| item.key);
        stable_sort_on_scheduler(&mut items, |a, b| a.key.cmp(&b.key), &scheduler);
        assert_eq!(items, expected);
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
