//! Private C ABI adapter for the semantics a generated artifact cannot compute for itself.
//!
//! Three of those so far: ECMAScript number conversion, whose shortest spelling no printf
//! conversion can express; regular expressions, which are an engine rather than a function; and
//! arbitrary-precision integers, which are exactly specified and still no easier to get right
//! twice (ADR 0025). All three are here for the same reason -- one implementation, shared with the
//! interpreter, rather than two that agree until they do not.
//!
//! The exported names are link-time implementation details of generated artifacts, not additions
//! to the public embedding ABI.
//!
//! The crate is still called `kiln-native-number-ffi`, which is now half a name. Renaming it moves
//! a packaging contract -- the manifest field, the archive filename, the environment variable --
//! and that is `kiln-native`'s to move; see NOTES.md.
#![deny(
    unsafe_op_in_unsafe_fn,
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

#[cfg(any(feature = "regexp", feature = "bigint"))]
extern crate alloc;

const INVALID_ARGUMENT: i32 = -1;
#[cfg(feature = "realm")]
const RUNTIME_ERROR: i32 = -8;

/// The private native Realm ABI.
///
/// Handles are process-unique integers rather than Rust pointers. The owning thread keeps the
/// actual runtime in thread-local storage, so a stale or cross-thread handle is rejected without
/// dereferencing caller-controlled memory. No Tokio or Rust layout crosses the C seam.
#[cfg(feature = "realm")]
mod realm {
    use super::{INVALID_ARGUMENT, RUNTIME_ERROR};
    use core::ffi::c_void;
    use kiln_runtime::{Realm, RealmRuntime, TimerId, WorkItem, duration_from_milliseconds};
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};

    type Entry = unsafe extern "C" fn(*mut c_void) -> i32;
    type RunWork = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;
    type DropWork = unsafe extern "C" fn(*mut c_void);

    struct WorkOwner {
        pointer: *mut c_void,
        run: RunWork,
        drop: DropWork,
    }

    impl Drop for WorkOwner {
        fn drop(&mut self) {
            // SAFETY: the enqueue call transfers one live work allocation and its matching
            // disposer. `Rc` makes this run exactly once, including interval cancellation.
            unsafe { (self.drop)(self.pointer) };
        }
    }

    #[derive(Clone)]
    struct NativeWork(Rc<WorkOwner>);

    impl NativeWork {
        fn new(pointer: *mut c_void, run: Option<RunWork>, drop: Option<DropWork>) -> Option<Self> {
            let (Some(run), Some(drop)) = (run, drop) else {
                return None;
            };
            if pointer.is_null() {
                return None;
            }
            Some(Self(Rc::new(WorkOwner { pointer, run, drop })))
        }

        fn run(&self, host: *mut c_void, thrown: *mut c_void) -> i32 {
            // SAFETY: the C Adapter supplied this callback with the allocation, and the caller of
            // `kiln_realm_drive` keeps `host` and `thrown` live for the complete drive.
            unsafe { (self.0.run)(self.0.pointer, host, thrown) }
        }
    }

    struct NativeRealm {
        runtime: RefCell<RealmRuntime>,
        work: RefCell<Realm<NativeWork, NativeWork>>,
        entered: Cell<bool>,
    }

    impl NativeRealm {
        fn new() -> Result<Self, kiln_runtime::RuntimeBuildError> {
            Ok(Self {
                runtime: RefCell::new(RealmRuntime::new()?),
                work: RefCell::new(Realm::new()),
                entered: Cell::new(false),
            })
        }
    }

    struct ActiveRealm {
        realm: Rc<NativeRealm>,
    }

    impl ActiveRealm {
        fn enter(handle: u64, realm: Rc<NativeRealm>) -> Option<Self> {
            if realm.entered.replace(true) {
                return None;
            }
            let activated = ACTIVE_REALM.with(|active| {
                if active.get() != 0 {
                    false
                } else {
                    active.set(handle);
                    true
                }
            });
            if !activated {
                realm.entered.set(false);
                return None;
            }
            Some(Self { realm })
        }
    }

    impl Drop for ActiveRealm {
        fn drop(&mut self) {
            ACTIVE_REALM.with(|active| active.set(0));
            self.realm.entered.set(false);
        }
    }

    static NEXT_REALM: AtomicU64 = AtomicU64::new(1);

    std::thread_local! {
        static REALMS: RefCell<HashMap<u64, Rc<NativeRealm>>> = RefCell::new(HashMap::new());
        static ACTIVE_REALM: Cell<u64> = const { Cell::new(0) };
    }

    fn ffi(operation: impl FnOnce() -> i32) -> i32 {
        catch_unwind(AssertUnwindSafe(operation)).unwrap_or(RUNTIME_ERROR)
    }

    fn realm(handle: u64) -> Option<Rc<NativeRealm>> {
        if handle == 0 {
            return None;
        }
        REALMS.with(|realms| realms.borrow().get(&handle).cloned())
    }

    fn active_realm() -> Option<Rc<NativeRealm>> {
        ACTIVE_REALM.with(|active| realm(active.get()))
    }

    fn drive(realm: &NativeRealm, host: *mut c_void, thrown: *mut c_void) -> i32 {
        loop {
            let microtask = realm.work.borrow_mut().pop_microtask();
            if let Some(work) = microtask {
                let status = work.run(host, thrown);
                if status != 0 {
                    return status;
                }
                continue;
            }

            let timer = realm
                .runtime
                .borrow_mut()
                .next_timer(&mut realm.work.borrow_mut());
            let Some(WorkItem::Timer { work, .. }) = timer else {
                return 0;
            };
            let status = work.run(host, thrown);
            if status != 0 {
                return status;
            }
        }
    }

    /// Creates one current-thread Realm Runtime and writes its opaque identity to `out`.
    ///
    /// # Safety
    ///
    /// `out` must be a writable `u64` slot owned by the caller.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_create(out: *mut u64) -> i32 {
        ffi(|| {
            if out.is_null() {
                return INVALID_ARGUMENT;
            }
            let Ok(realm) = NativeRealm::new() else {
                return RUNTIME_ERROR;
            };
            let Ok(handle) =
                NEXT_REALM.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                    next.checked_add(1)
                })
            else {
                return RUNTIME_ERROR;
            };
            REALMS.with(|realms| {
                realms.borrow_mut().insert(handle, Rc::new(realm));
            });
            // SAFETY: the caller supplies a writable slot and null was rejected above.
            unsafe { out.write(handle) };
            0
        })
    }

    /// Executes `entry(context)` as the current task of the owning Realm Runtime.
    ///
    /// Recursive entry fails closed instead of attempting a nested `block_on`.
    pub unsafe extern "C" fn enter(handle: u64, entry: Option<Entry>, context: *mut c_void) -> i32 {
        ffi(|| {
            let Some(entry) = entry else {
                return INVALID_ARGUMENT;
            };
            let Some(realm) = realm(handle) else {
                return INVALID_ARGUMENT;
            };
            let Some(_active) = ActiveRealm::enter(handle, Rc::clone(&realm)) else {
                return INVALID_ARGUMENT;
            };
            // SAFETY: the C caller owns `context` and the callback contract for this call.
            realm
                .runtime
                .borrow_mut()
                .enter(|| unsafe { entry(context) })
        })
    }

    /// C-exported spelling of [`enter`].
    ///
    /// # Safety
    ///
    /// `entry`, when present, must be a valid C function and `context` must satisfy that
    /// function's contract for the duration of this call.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_enter(
        handle: u64,
        entry: Option<Entry>,
        context: *mut c_void,
    ) -> i32 {
        // SAFETY: this function has exactly the same caller contract as `enter`.
        unsafe { enter(handle, entry, context) }
    }

    /// Returns one when generated code is already executing inside a Realm.
    #[unsafe(no_mangle)]
    pub extern "C" fn kiln_realm_is_active() -> i32 {
        i32::from(ACTIVE_REALM.with(|active| active.get() != 0))
    }

    /// Writes the active Realm's deterministic clock in ECMAScript milliseconds.
    ///
    /// # Safety
    ///
    /// `out_milliseconds` must be a writable `f64` slot owned by the caller.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_now(out_milliseconds: *mut f64) -> i32 {
        ffi(|| {
            if out_milliseconds.is_null() {
                return INVALID_ARGUMENT;
            }
            let Some(realm) = active_realm() else {
                return INVALID_ARGUMENT;
            };
            let Ok(work) = realm.work.try_borrow() else {
                return RUNTIME_ERROR;
            };
            let milliseconds = work.now().as_millis() as f64;
            // SAFETY: null was rejected and the caller supplies a writable slot.
            unsafe { out_milliseconds.write(milliseconds) };
            0
        })
    }

    /// Queues one opaque FIFO microtask on the active Realm.
    ///
    /// # Safety
    ///
    /// `work` must be owned storage accepted by `run` and `drop`. Ownership transfers on success.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_queue_microtask(
        run: Option<RunWork>,
        drop: Option<DropWork>,
        work: *mut c_void,
    ) -> i32 {
        ffi(|| {
            let Some(realm) = active_realm() else {
                return INVALID_ARGUMENT;
            };
            let Ok(mut queue) = realm.work.try_borrow_mut() else {
                return RUNTIME_ERROR;
            };
            let Some(work) = NativeWork::new(work, run, drop) else {
                return INVALID_ARGUMENT;
            };
            queue.queue_microtask(work);
            0
        })
    }

    fn set_timer(
        repeating: bool,
        delay_ms: f64,
        run: Option<RunWork>,
        drop: Option<DropWork>,
        work: *mut c_void,
        out_id: *mut u64,
    ) -> i32 {
        ffi(|| {
            if out_id.is_null() {
                return INVALID_ARGUMENT;
            }
            let Some(realm) = active_realm() else {
                return INVALID_ARGUMENT;
            };
            let Ok(mut queue) = realm.work.try_borrow_mut() else {
                return RUNTIME_ERROR;
            };
            let Some(work) = NativeWork::new(work, run, drop) else {
                return INVALID_ARGUMENT;
            };
            let delay = if delay_ms.is_finite() && delay_ms > 0.0 {
                duration_from_milliseconds(delay_ms)
            } else {
                std::time::Duration::ZERO
            };
            let id = if repeating {
                queue.set_interval(delay, work)
            } else {
                queue.set_timeout(delay, work)
            };
            // SAFETY: null was rejected and the caller supplies a writable identity slot.
            unsafe { out_id.write(id.get()) };
            0
        })
    }

    /// Schedules one opaque timeout on the active Realm.
    ///
    /// # Safety
    ///
    /// `out_id` must be writable and `work` must satisfy the callback ownership contract.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_set_timeout(
        delay_ms: f64,
        run: Option<RunWork>,
        drop: Option<DropWork>,
        work: *mut c_void,
        out_id: *mut u64,
    ) -> i32 {
        set_timer(false, delay_ms, run, drop, work, out_id)
    }

    /// Schedules one opaque interval on the active Realm.
    ///
    /// # Safety
    ///
    /// `out_id` must be writable and `work` must satisfy the callback ownership contract.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_set_interval(
        delay_ms: f64,
        run: Option<RunWork>,
        drop: Option<DropWork>,
        work: *mut c_void,
        out_id: *mut u64,
    ) -> i32 {
        set_timer(true, delay_ms, run, drop, work, out_id)
    }

    /// Cancels a timeout or interval on the active Realm. Unknown identities are a no-op.
    #[unsafe(no_mangle)]
    pub extern "C" fn kiln_realm_clear_timer(id: u64) -> i32 {
        ffi(|| {
            let Some(realm) = active_realm() else {
                return INVALID_ARGUMENT;
            };
            let Ok(mut queue) = realm.work.try_borrow_mut() else {
                return RUNTIME_ERROR;
            };
            queue.clear_timeout(TimerId::new(id));
            0
        })
    }

    /// Drives every queued microtask and timer on `handle` to deterministic quiescence.
    ///
    /// # Safety
    ///
    /// `host` and `thrown` must remain valid for every queued work callback during this call.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_drive(
        handle: u64,
        host: *mut c_void,
        thrown: *mut c_void,
    ) -> i32 {
        ffi(|| {
            let Some(realm) = realm(handle) else {
                return INVALID_ARGUMENT;
            };
            let Some(_active) = ActiveRealm::enter(handle, Rc::clone(&realm)) else {
                return INVALID_ARGUMENT;
            };
            drive(&realm, host, thrown)
        })
    }

    /// Destroys a Realm Runtime on its owning thread.
    #[unsafe(no_mangle)]
    pub extern "C" fn kiln_realm_destroy(handle: u64) -> i32 {
        ffi(|| {
            if handle == 0 {
                return INVALID_ARGUMENT;
            }
            if ACTIVE_REALM.with(|active| active.get() == handle) {
                return INVALID_ARGUMENT;
            }
            REALMS.with(|realms| {
                if realms.borrow_mut().remove(&handle).is_some() {
                    0
                } else {
                    INVALID_ARGUMENT
                }
            })
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::ptr;

        struct RecordedWork {
            value: u8,
            order: *mut Vec<u8>,
            drops: *mut usize,
            clear: u64,
        }

        unsafe extern "C" fn record_work(
            work: *mut c_void,
            _host: *mut c_void,
            _thrown: *mut c_void,
        ) -> i32 {
            if work.is_null() {
                return INVALID_ARGUMENT;
            }
            let work = work.cast::<RecordedWork>();
            // SAFETY: the scheduling test keeps the record and both target allocations live until
            // Realm teardown invokes `drop_recorded_work`.
            unsafe {
                (*(*work).order).push((*work).value);
                if (*work).clear != 0 {
                    return kiln_realm_clear_timer((*work).clear);
                }
            }
            0
        }

        unsafe extern "C" fn drop_recorded_work(work: *mut c_void) {
            if work.is_null() {
                return;
            }
            // SAFETY: Realm ownership invokes this exactly once for the pointer transferred by
            // the scheduling test.
            let work = unsafe { Box::from_raw(work.cast::<RecordedWork>()) };
            // SAFETY: the test's drop counter outlives Realm teardown.
            unsafe { *work.drops = (*work.drops).saturating_add(1) };
        }

        fn recorded_work(value: u8, order: &mut Vec<u8>, drops: &mut usize) -> *mut c_void {
            Box::into_raw(Box::new(RecordedWork {
                value,
                order,
                drops,
                clear: 0,
            }))
            .cast::<c_void>()
        }

        unsafe extern "C" fn schedule_recorded_work(context: *mut c_void) -> i32 {
            if context.is_null() {
                return INVALID_ARGUMENT;
            }
            // SAFETY: the test passes live `(Vec<u8>, usize)` storage for this entry call.
            let (order, drops) = unsafe { &mut *context.cast::<(Vec<u8>, usize)>() };
            let microtask = recorded_work(1, order, drops);
            // SAFETY: callbacks match the transferred allocation.
            let mut status = unsafe {
                kiln_realm_queue_microtask(Some(record_work), Some(drop_recorded_work), microtask)
            };
            if status != 0 {
                // SAFETY: ownership did not transfer on failure.
                unsafe { drop_recorded_work(microtask) };
                return status;
            }

            for (value, delay) in [(2, 0.0), (4, 10.0)] {
                let timer = recorded_work(value, order, drops);
                let mut id = 0;
                // SAFETY: callbacks, allocation, and identity slot satisfy the timer contract.
                status = unsafe {
                    kiln_realm_set_timeout(
                        delay,
                        Some(record_work),
                        Some(drop_recorded_work),
                        timer,
                        &raw mut id,
                    )
                };
                if status != 0 {
                    // SAFETY: ownership did not transfer on failure.
                    unsafe { drop_recorded_work(timer) };
                    return status;
                }
            }

            let interval = recorded_work(3, order, drops);
            let mut interval_id = 0;
            // SAFETY: callbacks, allocation, and identity slot satisfy the interval contract.
            status = unsafe {
                kiln_realm_set_interval(
                    5.0,
                    Some(record_work),
                    Some(drop_recorded_work),
                    interval,
                    &raw mut interval_id,
                )
            };
            if status != 0 {
                // SAFETY: ownership did not transfer on failure.
                unsafe { drop_recorded_work(interval) };
                return status;
            }
            // SAFETY: the allocation remains Realm-owned and live until the interval is cleared.
            unsafe { (*interval.cast::<RecordedWork>()).clear = interval_id };
            0
        }

        unsafe extern "C" fn sees_tokio(context: *mut c_void) -> i32 {
            if context.is_null() || tokio::runtime::Handle::try_current().is_err() {
                return RUNTIME_ERROR;
            }
            // SAFETY: the test passes a live `bool` slot.
            unsafe { context.cast::<bool>().write(true) };
            17
        }

        unsafe extern "C" fn reads_realm_clock(context: *mut c_void) -> i32 {
            // SAFETY: the test supplies one live `f64` slot while this callback runs.
            unsafe { kiln_realm_now(context.cast::<f64>()) }
        }

        #[test]
        fn opaque_handle_enters_tokio_and_rejects_stale_or_cross_thread_use() {
            let mut handle = 0;
            // SAFETY: `handle` is a live writable slot.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            assert_ne!(handle, 0);
            let mut entered = false;
            // SAFETY: callback and context satisfy `sees_tokio`'s contract.
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(sees_tokio),
                        (&raw mut entered).cast::<c_void>(),
                    )
                },
                17
            );
            assert!(entered);

            let foreign = std::thread::spawn(move || {
                // SAFETY: null context is accepted by this callback only after handle lookup;
                // the foreign thread must reject the handle before invoking it.
                unsafe { kiln_realm_enter(handle, Some(sees_tokio), ptr::null_mut()) }
            })
            .join()
            .unwrap_or(RUNTIME_ERROR);
            assert_eq!(foreign, INVALID_ARGUMENT);
            assert_eq!(kiln_realm_destroy(handle), 0);
            assert_eq!(kiln_realm_destroy(handle), INVALID_ARGUMENT);
        }

        #[test]
        fn null_slots_and_callbacks_fail_closed() {
            // SAFETY: the null pointer is intentionally rejected without dereferencing.
            assert_eq!(
                unsafe { kiln_realm_create(ptr::null_mut()) },
                INVALID_ARGUMENT
            );
            // SAFETY: the absent callback is intentionally rejected.
            assert_eq!(
                unsafe { kiln_realm_enter(1, None, ptr::null_mut()) },
                INVALID_ARGUMENT
            );
            // SAFETY: the null slot is intentionally rejected without dereferencing.
            assert_eq!(unsafe { kiln_realm_now(ptr::null_mut()) }, INVALID_ARGUMENT);
        }

        #[test]
        fn realm_clock_is_visible_only_while_entered() {
            let mut handle = 0;
            let mut now = f64::NAN;
            // SAFETY: handle and clock slots remain live for their calls.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(reads_realm_clock),
                        (&raw mut now).cast::<c_void>(),
                    )
                },
                0
            );
            assert_eq!(now, 0.0);
            // SAFETY: the slot is live, but no Realm is active now.
            assert_eq!(unsafe { kiln_realm_now(&raw mut now) }, INVALID_ARGUMENT);
            assert_eq!(kiln_realm_destroy(handle), 0);
        }

        #[test]
        fn rust_panics_become_runtime_errors_before_reaching_c() {
            assert_eq!(
                ffi(|| std::panic::resume_unwind(Box::new("realm panic"))),
                RUNTIME_ERROR
            );
        }

        #[test]
        fn opaque_work_uses_realm_ordering_and_drops_every_payload_once() {
            let mut handle = 0;
            let mut observed = (Vec::<u8>::new(), 0_usize);
            // SAFETY: handle and context are live for the calls that use them.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(schedule_recorded_work),
                        (&raw mut observed).cast::<c_void>(),
                    )
                },
                0
            );
            let mut host = 0_u8;
            let mut thrown = 0_u8;
            // SAFETY: both opaque callback-context slots remain live for the complete drive.
            assert_eq!(
                unsafe {
                    kiln_realm_drive(
                        handle,
                        (&raw mut host).cast::<c_void>(),
                        (&raw mut thrown).cast::<c_void>(),
                    )
                },
                0
            );
            assert_eq!(observed.0, [1, 2, 3, 4]);
            assert_eq!(kiln_realm_destroy(handle), 0);
            assert_eq!(observed.1, 4);
        }
    }
}

/// The allocator the regexp engine runs on.
///
/// `malloc` and `free`, because the generated artifact this is linked into already has them and a
/// second allocator would be a second heap in one process. Alignment above `max_align_t` goes
/// through `posix_memalign`, which is the only part `malloc` cannot promise.
#[cfg(all(not(test), any(feature = "regexp", feature = "bigint")))]
mod heap {
    use core::alloc::{GlobalAlloc, Layout};

    unsafe extern "C" {
        fn malloc(size: usize) -> *mut core::ffi::c_void;
        fn free(pointer: *mut core::ffi::c_void);
        fn posix_memalign(
            out: *mut *mut core::ffi::c_void,
            alignment: usize,
            size: usize,
        ) -> core::ffi::c_int;
    }

    /// What `malloc` already guarantees on every target this compiles for.
    const MALLOC_ALIGNMENT: usize = 16;

    struct CHeap;

    // SAFETY: every allocation is returned by `malloc` or `posix_memalign` and freed by `free`,
    // which is the contract both halves of libc's allocator keep.
    unsafe impl GlobalAlloc for CHeap {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if layout.align() <= MALLOC_ALIGNMENT {
                // SAFETY: a nonzero size is guaranteed by the `Layout` contract.
                return unsafe { malloc(layout.size()) }.cast::<u8>();
            }
            let mut out = core::ptr::null_mut();
            // SAFETY: `out` is a valid slot for the returned pointer.
            let status = unsafe { posix_memalign(&raw mut out, layout.align(), layout.size()) };
            if status == 0 {
                out.cast::<u8>()
            } else {
                core::ptr::null_mut()
            }
        }

        unsafe fn dealloc(&self, pointer: *mut u8, _layout: Layout) {
            // SAFETY: the pointer came from `alloc` above and is freed exactly once.
            unsafe { free(pointer.cast::<core::ffi::c_void>()) };
        }
    }

    #[global_allocator]
    static HEAP: CHeap = CHeap;
}

/// Format `value` using ECMAScript `Number::toString` spelling.
///
/// On success the function returns the byte length excluding a trailing NUL and writes that NUL.
/// A null output, zero capacity, or a buffer without room for both the bytes and terminator returns
/// `-1` without modifying the output.
///
/// # Safety
///
/// When `buffer` is non-null, it must name storage valid for writes of `capacity` bytes for the
/// duration of this call. The storage must not overlap memory used by this function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_number_to_js_string(
    value: f64,
    buffer: *mut u8,
    capacity: usize,
) -> i32 {
    if buffer.is_null() || capacity == 0 {
        return INVALID_ARGUMENT;
    }

    let mut formatted = [0_u8; kiln_number_core::MAX_FORMATTED_LEN];
    let Ok(length) = kiln_number_core::format_number(value, &mut formatted) else {
        return INVALID_ARGUMENT;
    };
    let Some(required) = length.checked_add(1) else {
        return INVALID_ARGUMENT;
    };
    if capacity < required {
        return INVALID_ARGUMENT;
    }
    let Some(bytes) = formatted.get(..length) else {
        return INVALID_ARGUMENT;
    };
    let Ok(result) = i32::try_from(length) else {
        return INVALID_ARGUMENT;
    };

    // SAFETY: the caller contract guarantees `capacity` writable bytes, the check above proves
    // `length + 1 <= capacity`, and `formatted` is disjoint stack storage.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, length);
        buffer.add(length).write(0);
    }
    result
}

/// Parse an ECMAScript `StringNumericLiteral` from a byte span.
///
/// `(NULL, 0)` is the empty string and produces positive zero. A null pointer with a nonzero length,
/// a span too large for Rust's slice contract, invalid UTF-8, or malformed input produces `NaN`.
///
/// # Safety
///
/// When `bytes` is non-null and `length` is nonzero, it must name readable storage of exactly
/// `length` bytes for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_string_to_number(bytes: *const u8, length: usize) -> f64 {
    if length == 0 {
        return 0.0;
    }
    if bytes.is_null() || length > isize::MAX as usize {
        return f64::NAN;
    }

    // SAFETY: the caller contract supplies `length` readable bytes, null was rejected, `u8` has
    // alignment one, and the explicit bound satisfies `slice::from_raw_parts`' size requirement.
    let input = unsafe { core::slice::from_raw_parts(bytes, length) };
    kiln_number_core::string_to_number(input)
}

#[cfg(feature = "regexp")]
mod regexp {
    use super::INVALID_ARGUMENT;

    /// A compiled pattern, and the last match it produced.
    ///
    /// The match is kept because a capture group's *name* is only reachable through one -- `regress`
    /// clones the names into each match rather than exposing them on the pattern. Keeping it also
    /// means the C side reads a result with two calls instead of marshalling a variable-length
    /// structure across the boundary.
    pub struct KilnRegex {
        compiled: regress::Regex,
        last: Option<regress::Match>,
    }

    /// Compile `pattern` with `flags`.
    ///
    /// Returns 0 and writes the handle on success, and -1 for a pattern the engine rejects -- which
    /// the caller turns into a SyntaxError, because that is what `new RegExp` with the same text
    /// produces.
    ///
    /// # Safety
    ///
    /// `pattern` and `flags` must name readable UTF-8 spans of the given lengths, and `out_handle`
    /// must be a writable slot. The handle belongs to the caller and is released by
    /// [`kiln_regexp_free`].
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_regexp_compile(
        pattern: *const u8,
        pattern_len: usize,
        flags: *const u8,
        flags_len: usize,
        out_handle: *mut *mut KilnRegex,
    ) -> i32 {
        if out_handle.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees both spans are readable for their lengths; a null
        // pointer with a zero length is the empty span, which `from_raw_parts` requires be dangling
        // rather than null.
        let (pattern, flags) = unsafe { (span(pattern, pattern_len), span(flags, flags_len)) };
        let (Some(pattern), Some(flags)) = (pattern, flags) else {
            return INVALID_ARGUMENT;
        };
        let (Ok(pattern), Ok(flags)) = (core::str::from_utf8(pattern), core::str::from_utf8(flags))
        else {
            return INVALID_ARGUMENT;
        };
        let Ok(compiled) = regress::Regex::with_flags(pattern, flags) else {
            return INVALID_ARGUMENT;
        };
        let handle = alloc::boxed::Box::new(KilnRegex {
            compiled,
            last: None,
        });
        // SAFETY: `out_handle` is a writable slot by the caller contract.
        unsafe { out_handle.write(alloc::boxed::Box::into_raw(handle)) };
        0
    }

    /// Release a handle from [`kiln_regexp_compile`].
    ///
    /// # Safety
    ///
    /// `handle` must have come from [`kiln_regexp_compile`] and must not be used again.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_regexp_free(handle: *mut KilnRegex) {
        if handle.is_null() {
            return;
        }
        // SAFETY: the caller contract guarantees the pointer came from `Box::into_raw` above.
        drop(unsafe { alloc::boxed::Box::from_raw(handle) });
    }

    /// Search `subject` from `start`, both counted in UTF-16 code units.
    ///
    /// Returns 1 for a match, 0 for none, and -1 for an invalid argument. On a match, up to
    /// `out_capacity` pairs of `(start, end)` offsets are written to `out_ranges`, one pair per group
    /// beginning with the whole match; a group that did not participate writes `(-1, -1)`.
    ///
    /// The offsets are UTF-16 code units because that is what the language counts in (ADR 0022), and
    /// matching over UTF-16 is what makes them so without a conversion.
    ///
    /// # Safety
    ///
    /// `handle` must be live, `subject` must name a readable span of `subject_len` units, and
    /// `out_ranges` must be writable for `out_capacity * 2` values.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_regexp_search(
        handle: *mut KilnRegex,
        subject: *const u16,
        subject_len: usize,
        start: usize,
        out_ranges: *mut i64,
        out_capacity: usize,
    ) -> i32 {
        if handle.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the handle is live and uniquely borrowed here.
        let state = unsafe { &mut *handle };
        if start > subject_len {
            state.last = None;
            return 0;
        }
        // SAFETY: the caller contract guarantees the span is readable for its length.
        let Some(units) = (unsafe { units(subject, subject_len) }) else {
            return INVALID_ARGUMENT;
        };
        let Some(found) = state.compiled.find_from_utf16(units, start).next() else {
            state.last = None;
            return 0;
        };
        let mut written = 0usize;
        for group in found.groups() {
            if written >= out_capacity || out_ranges.is_null() {
                break;
            }
            let (from, to) = match group {
                Some(range) => (as_offset(range.start), as_offset(range.end)),
                // Not zero: a group that did not participate is `undefined`, which is a different
                // answer from one that matched nothing.
                None => (-1, -1),
            };
            // SAFETY: `written` is below `out_capacity`, and the caller promised twice that many
            // writable values.
            unsafe {
                out_ranges.add(written.wrapping_mul(2)).write(from);
                out_ranges
                    .add(written.wrapping_mul(2).wrapping_add(1))
                    .write(to);
            }
            written = written.wrapping_add(1);
        }
        state.last = Some(found);
        1
    }

    /// How many groups the last match reported, including the whole match as group zero.
    ///
    /// # Safety
    ///
    /// `handle` must be live.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_regexp_group_count(handle: *const KilnRegex) -> usize {
        if handle.is_null() {
            return 0;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        let state = unsafe { &*handle };
        state
            .last
            .as_ref()
            .map_or(0, |found| found.groups().count())
    }

    /// The name of capture group `index` from the last match, as UTF-8 without a terminator.
    ///
    /// Returns the byte length written, 0 for a group with no name, and -1 when there is no last match
    /// or the buffer is too small.
    ///
    /// # Safety
    ///
    /// `handle` must be live and `buffer` must be writable for `capacity` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_regexp_group_name(
        handle: *const KilnRegex,
        index: usize,
        buffer: *mut u8,
        capacity: usize,
    ) -> i32 {
        if handle.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        let state = unsafe { &*handle };
        let Some(found) = state.last.as_ref() else {
            return INVALID_ARGUMENT;
        };
        // `named_groups` counts from the first capture group, where the caller counts from the whole
        // match -- so group zero has no name by construction and the rest are shifted by one.
        let Some(wanted) = index.checked_sub(1) else {
            return 0;
        };
        let Some((name, _)) = found.named_groups().nth(wanted) else {
            return 0;
        };
        let bytes = name.as_bytes();
        let Ok(length) = i32::try_from(bytes.len()) else {
            return INVALID_ARGUMENT;
        };
        if bytes.len() > capacity || buffer.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the length is within the capacity the caller promised, and `name` is disjoint from
        // the caller's buffer.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
        length
    }

    /// A byte span, or `None` when the pointer contract is violated.
    ///
    /// # Safety
    ///
    /// A non-null `pointer` must be readable for `length` bytes.
    unsafe fn span<'a>(pointer: *const u8, length: usize) -> Option<&'a [u8]> {
        if length == 0 {
            return Some(&[]);
        }
        if pointer.is_null() || length > isize::MAX as usize {
            return None;
        }
        // SAFETY: checked above and guaranteed by the caller contract.
        Some(unsafe { core::slice::from_raw_parts(pointer, length) })
    }

    /// The same, for the UTF-16 subject.
    ///
    /// # Safety
    ///
    /// A non-null `pointer` must be readable for `length` units.
    unsafe fn units<'a>(pointer: *const u16, length: usize) -> Option<&'a [u16]> {
        if length == 0 {
            return Some(&[]);
        }
        if pointer.is_null() || length > isize::MAX as usize / 2 {
            return None;
        }
        // SAFETY: checked above and guaranteed by the caller contract.
        Some(unsafe { core::slice::from_raw_parts(pointer, length) })
    }

    /// An offset that fits the boundary type, saturating rather than wrapping.
    fn as_offset(value: usize) -> i64 {
        i64::try_from(value).unwrap_or(i64::MAX)
    }
}

/// Arbitrary-precision integers, exactly as the interpreter holds them.
///
/// A BigInt exists to be exact, so the one thing this must never do is lose a digit. Handing the
/// work to `num-bigint` -- the crate the interpreter already links -- is what makes "the two
/// engines agree" a property of the build rather than of the test suite.
//
// The crate denies `arithmetic_side_effects` because a fixed-width overflow is a silent wrong
// answer. Arbitrary-precision arithmetic has no such failure -- it grows instead -- and the two
// operations that *can* fail, division by zero and an exponent no machine can hold, are checked
// before they run.
#[expect(clippy::arithmetic_side_effects)]
#[cfg(feature = "bigint")]
mod bigint {
    use super::INVALID_ARGUMENT;
    use alloc::boxed::Box;
    use num_bigint::BigInt;
    use num_traits::{FromPrimitive, Pow, Signed, ToPrimitive, Zero};

    /// An operation the language defines but this cannot perform: division by zero, or an
    /// exponent no machine could represent. The caller turns it into a RangeError.
    const OUT_OF_RANGE: i32 = -2;

    /// The binary operators, numbered. Stable only within one generated unit, like every other
    /// operation code in the emitted runtime.
    const ADD: u32 = 1;
    const SUBTRACT: u32 = 2;
    const MULTIPLY: u32 = 3;
    const DIVIDE: u32 = 4;
    const REMAINDER: u32 = 5;
    const EXPONENTIATE: u32 = 6;
    const BITWISE_AND: u32 = 7;
    const BITWISE_OR: u32 = 8;
    const BITWISE_XOR: u32 = 9;

    /// Parse a digit string in `radix`, with an optional leading sign.
    ///
    /// # Safety
    ///
    /// `text` must name a readable span of `len` bytes when `len` is nonzero.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_parse(
        text: *const u8,
        len: usize,
        radix: u32,
        out: *mut *mut BigInt,
    ) -> i32 {
        if out.is_null() || !(2..=36).contains(&radix) {
            return INVALID_ARGUMENT;
        }
        if len == 0 {
            return INVALID_ARGUMENT;
        }
        if text.is_null() || len > isize::MAX as usize {
            return INVALID_ARGUMENT;
        }
        // SAFETY: checked above and guaranteed by the caller contract.
        let bytes = unsafe { core::slice::from_raw_parts(text, len) };
        let Ok(digits) = core::str::from_utf8(bytes) else {
            return INVALID_ARGUMENT;
        };
        let Some(parsed) = BigInt::parse_bytes(digits.trim().as_bytes(), radix) else {
            return INVALID_ARGUMENT;
        };
        // SAFETY: `out` is a writable slot by the caller contract.
        unsafe { out.write(Box::into_raw(Box::new(parsed))) };
        0
    }

    /// `BigInt(n)`, which requires an integral finite number: rounding here would be the one thing
    /// a BigInt exists to prevent.
    ///
    /// # Safety
    ///
    /// `out` must be a writable slot.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_from_double(value: f64, out: *mut *mut BigInt) -> i32 {
        if out.is_null() {
            return INVALID_ARGUMENT;
        }
        // `trunc` rather than `fract`: `num_traits` brings its own `fract` into scope here and
        // the two disagree about nothing except which one the compiler picks.
        if !value.is_finite() || value != libm_trunc(value) {
            return OUT_OF_RANGE;
        }
        let Some(converted) = BigInt::from_f64(value) else {
            return OUT_OF_RANGE;
        };
        // SAFETY: `out` is a writable slot by the caller contract.
        unsafe { out.write(Box::into_raw(Box::new(converted))) };
        0
    }

    /// # Safety
    ///
    /// `handle` must have come from one of the constructors above and must not be used again.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_free(handle: *mut BigInt) {
        if handle.is_null() {
            return;
        }
        // SAFETY: the caller contract guarantees the pointer came from `Box::into_raw`.
        drop(unsafe { Box::from_raw(handle) });
    }

    /// The nearest double, which is what a comparison against a number needs -- and is lossy by
    /// definition, which is why nothing else uses it.
    ///
    /// # Safety
    ///
    /// `handle` must be live.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_to_double(handle: *const BigInt) -> f64 {
        if handle.is_null() {
            return f64::NAN;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        unsafe { &*handle }.to_f64().unwrap_or(f64::NAN)
    }

    /// Whether the value is zero, which is the whole of a BigInt's truthiness.
    ///
    /// # Safety
    ///
    /// `handle` must be live.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_is_zero(handle: *const BigInt) -> i32 {
        if handle.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        i32::from(unsafe { &*handle }.is_zero())
    }

    /// `-1`, `0` or `1`, or `-2` when either handle is null.
    ///
    /// # Safety
    ///
    /// Both handles must be live.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_compare(left: *const BigInt, right: *const BigInt) -> i32 {
        if left.is_null() || right.is_null() {
            return OUT_OF_RANGE;
        }
        // SAFETY: the caller contract guarantees both handles are live.
        let (left, right) = unsafe { (&*left, &*right) };
        match left.cmp(right) {
            core::cmp::Ordering::Less => -1,
            core::cmp::Ordering::Equal => 0,
            core::cmp::Ordering::Greater => 1,
        }
    }

    /// The value written in `radix`, as UTF-8 without a terminator, or the required length as a
    /// negative number when the buffer is too small.
    ///
    /// # Safety
    ///
    /// `handle` must be live and `buffer` must be writable for `capacity` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_to_string(
        handle: *const BigInt,
        radix: u32,
        buffer: *mut u8,
        capacity: usize,
    ) -> i32 {
        if handle.is_null() || !(2..=36).contains(&radix) {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        let text = unsafe { &*handle }.to_str_radix(radix);
        let bytes = text.as_bytes();
        let Ok(length) = i32::try_from(bytes.len()) else {
            return INVALID_ARGUMENT;
        };
        if bytes.len() > capacity || buffer.is_null() {
            // Negative, so the caller can size a buffer and ask again rather than guessing.
            return length.saturating_neg();
        }
        // SAFETY: the length is within the capacity the caller promised.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
        length
    }

    /// One binary operation.
    ///
    /// # Safety
    ///
    /// Both handles must be live and `out` must be a writable slot.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_binary(
        operation: u32,
        left: *const BigInt,
        right: *const BigInt,
        out: *mut *mut BigInt,
    ) -> i32 {
        if left.is_null() || right.is_null() || out.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees both handles are live.
        let (left, right) = unsafe { (&*left, &*right) };
        let produced = match operation {
            ADD => left + right,
            SUBTRACT => left - right,
            MULTIPLY => left * right,
            // Division truncates toward zero and by zero is a RangeError, neither of which
            // matches what a number would do.
            DIVIDE | REMAINDER => {
                if right.is_zero() {
                    return OUT_OF_RANGE;
                }
                if operation == DIVIDE {
                    left / right
                } else {
                    left % right
                }
            }
            EXPONENTIATE => {
                if right.is_negative() {
                    return OUT_OF_RANGE;
                }
                let Some(exponent) = right.to_u32() else {
                    return OUT_OF_RANGE;
                };
                Pow::pow(left.clone(), exponent)
            }
            BITWISE_AND => left & right,
            BITWISE_OR => left | right,
            BITWISE_XOR => left ^ right,
            _ => return INVALID_ARGUMENT,
        };
        // SAFETY: `out` is a writable slot by the caller contract.
        unsafe { out.write(Box::into_raw(Box::new(produced))) };
        0
    }

    /// `trunc` without pulling in a math library: a double's integral part.
    fn libm_trunc(value: f64) -> f64 {
        // `as i64` would saturate for anything large, which is exactly the case that matters.
        if value.abs() >= 9_007_199_254_740_992.0 {
            return value;
        }
        #[expect(clippy::cast_possible_truncation)]
        let whole = value as i64;
        #[expect(clippy::cast_precision_loss)]
        let whole = whole as f64;
        whole
    }

    /// `-x`, which is the only unary operation a BigInt has that is not a comparison.
    ///
    /// # Safety
    ///
    /// `handle` must be live and `out` must be a writable slot.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_negate(
        handle: *const BigInt,
        out: *mut *mut BigInt,
    ) -> i32 {
        if handle.is_null() || out.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        let negated = -(unsafe { &*handle }).clone();
        // SAFETY: `out` is a writable slot by the caller contract.
        unsafe { out.write(Box::into_raw(Box::new(negated))) };
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatter_translates_capacity_and_nul_contracts_without_partial_writes() {
        let mut exact = [b'X'; 6];
        // SAFETY: `exact` is writable for its full reported capacity.
        let length = unsafe { kiln_number_to_js_string(1.234, exact.as_mut_ptr(), exact.len()) };
        assert_eq!(length, 5);
        assert_eq!(&exact, b"1.234\0");

        let mut short = [b'X'; 5];
        // SAFETY: `short` is writable for its full reported capacity.
        let length = unsafe { kiln_number_to_js_string(1.234, short.as_mut_ptr(), short.len()) };
        assert_eq!(length, INVALID_ARGUMENT);
        assert_eq!(&short, b"XXXXX");

        // SAFETY: a null pointer is explicitly accepted as an invalid argument and never read.
        assert_eq!(
            unsafe { kiln_number_to_js_string(1.0, core::ptr::null_mut(), 0) },
            INVALID_ARGUMENT
        );
    }

    #[test]
    fn parser_maps_null_empty_and_bounded_bytes_to_the_existing_contract() {
        // SAFETY: `(NULL, 0)` is explicitly the empty span.
        let empty = unsafe { kiln_string_to_number(core::ptr::null(), 0) };
        assert_eq!(empty.to_bits(), 0.0_f64.to_bits());

        // SAFETY: null with nonzero length is rejected before it can be read.
        assert!(unsafe { kiln_string_to_number(core::ptr::null(), 1) }.is_nan());

        let text = b"0x20000000000003";
        // SAFETY: `text` is readable for its reported length.
        let parsed = unsafe { kiln_string_to_number(text.as_ptr(), text.len()) };
        assert_eq!(parsed, 9_007_199_254_740_996.0);

        let malformed = [0xc0, 0xaf];
        // SAFETY: `malformed` is readable for its reported length.
        assert!(unsafe { kiln_string_to_number(malformed.as_ptr(), malformed.len()) }.is_nan());
    }
}
