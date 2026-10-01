use std::error::Error;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

static CALLABLE_STORE_DROPS: AtomicUsize = AtomicUsize::new(0);

fn opaque_test_pointer(identity: usize) -> *mut c_void {
    ptr::without_provenance_mut(identity)
}

unsafe extern "C" fn record_callable_store_drop(_pointer: *mut c_void) {
    CALLABLE_STORE_DROPS.fetch_add(1, AtomicOrdering::SeqCst);
}

use super::*;
use std::fs;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn scheduled_native_realms_keep_distinct_worker_programs_on_one_lane() {
    struct Turn {
        port: u64,
        exit: bool,
        observed_exit: Option<i32>,
    }
    unsafe extern "C" fn enter(context: *mut c_void) -> i32 {
        let turn = unsafe { &mut *context.cast::<Turn>() };
        if turn.port == 0 {
            turn.port = kiln_runtime::workers::broadcast_join("scheduled-native-program");
        }
        if turn.exit {
            kiln_runtime::workers::own_exits();
            assert!(kiln_runtime::workers::request_exit(23));
        } else {
            turn.observed_exit = kiln_runtime::workers::take_exit_request();
        }
        0
    }

    let scheduler = Arc::new(PinnedTurnScheduler::new(1).expect("one lane"));
    let _pool = ScheduledPoolGuard::enter(Some(Arc::downgrade(&scheduler)));
    let mut first = 0;
    let mut second = 0;
    assert_eq!(unsafe { kiln_realm_create(&raw mut first) }, 0);
    assert_eq!(unsafe { kiln_realm_create(&raw mut second) }, 0);
    let mut a = Turn {
        port: 0,
        exit: true,
        observed_exit: None,
    };
    let mut b = Turn {
        port: 0,
        exit: false,
        observed_exit: None,
    };
    assert_eq!(
        unsafe { kiln_realm_enter(first, Some(enter), (&raw mut a).cast()) },
        0
    );
    assert_eq!(
        unsafe { kiln_realm_enter(second, Some(enter), (&raw mut b).cast()) },
        0
    );
    assert_eq!(b.observed_exit, None);
    assert_eq!(
        kiln_runtime::workers::broadcast_post(
            a.port,
            kiln_runtime::workers::Message::new(kiln_runtime::workers::kind::USER, Vec::new()),
        ),
        0,
    );
    assert!(kiln_runtime::workers::take(b.port).is_none());
    a.exit = false;
    assert_eq!(
        unsafe { kiln_realm_enter(first, Some(enter), (&raw mut a).cast()) },
        0
    );
    assert_eq!(a.observed_exit, Some(23));
    kiln_runtime::workers::close(a.port);
    kiln_runtime::workers::close(b.port);
    assert_eq!(kiln_realm_destroy(first), 0);
    assert_eq!(kiln_realm_destroy(second), 0);
}

#[test]
fn unpinned_realm_rebinds_its_executor_on_another_thread() {
    static LANE_RESETS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn roots(_heap: *mut c_void, _context: *mut c_void, phase: i32) {
        match phase {
            GC_PHASE_LANE_RESET => {
                LANE_RESETS.fetch_add(1, AtomicOrdering::SeqCst);
            }
            GC_PHASE_RESET => {}
            _ => panic!("unexpected root phase"),
        }
    }
    unsafe extern "C" fn register_roots(_context: *mut c_void) -> i32 {
        unsafe { kiln_realm_gc_register_roots(Some(roots), ptr::null_mut(), 0) }
    }
    unsafe extern "C" fn read_clock(context: *mut c_void) -> i32 {
        unsafe { kiln_realm_now(context.cast()) }
    }

    LANE_RESETS.store(0, AtomicOrdering::SeqCst);
    let reactor = kiln_runtime::Reactor::shared_wall().expect("shared reactor");
    let _scheduled = ScheduledReactorGuard::enter(reactor);
    let mut handle = 0;
    assert_eq!(unsafe { kiln_realm_create(&mut handle) }, 0);
    assert_eq!(kiln_realm_use_wall_clock(handle), 0);
    assert_eq!(
        unsafe { kiln_realm_enter(handle, Some(register_roots), ptr::null_mut()) },
        0
    );
    let detached = detach_realm(handle).expect("realm has no affine work");
    assert!(
        realm(handle).is_none(),
        "source lane released the registry entry"
    );
    let reading = std::thread::spawn(move || {
        attach_realm(handle, detached);
        let mut now = 0.0;
        assert_eq!(
            unsafe { kiln_realm_enter(handle, Some(read_clock), (&raw mut now).cast()) },
            0
        );
        assert_eq!(kiln_realm_destroy(handle), 0);
        now
    })
    .join()
    .expect("destination lane completes");
    assert!(reading > 0.0);
    assert_eq!(LANE_RESETS.load(AtomicOrdering::SeqCst), 2);
}

struct ScheduledFixture {
    realm: u64,
}

unsafe extern "C" fn scheduled_fixture_begin(context: *mut c_void, _index: u64) -> *mut c_void {
    let lanes =
        unsafe { &*context.cast::<std::sync::Arc<std::sync::Mutex<Vec<std::thread::ThreadId>>>>() };
    let mut realm = 0;
    if unsafe { kiln_realm_create(&mut realm) } != 0 || kiln_realm_use_wall_clock(realm) != 0 {
        return ptr::null_mut();
    }
    lanes.lock().unwrap().push(std::thread::current().id());
    Box::into_raw(Box::new(ScheduledFixture { realm })).cast()
}

unsafe extern "C" fn scheduled_fixture_advance(session: *mut c_void, delay: *mut u64) -> i32 {
    let session = unsafe { &*session.cast::<ScheduledFixture>() };
    let mut kind = 3;
    let status = unsafe {
        kiln_realm_drive_step(
            session.realm,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut kind,
            delay,
        )
    };
    if status != 0 {
        return 0;
    }
    match kind {
        0 => 1,
        1 => 2,
        2 => 3,
        3 => 0,
        _ => -1,
    }
}

unsafe extern "C" fn scheduled_fixture_finish(session: *mut c_void) -> i32 {
    let session = unsafe { Box::from_raw(session.cast::<ScheduledFixture>()) };
    kiln_realm_destroy(session.realm)
}

#[test]
fn scheduled_provider_runs_many_realms_over_two_lanes() {
    let lanes = std::sync::Arc::new(std::sync::Mutex::new(Vec::<std::thread::ThreadId>::new()));
    let mut statuses = vec![-1; 200];
    assert_eq!(
        unsafe {
            kiln_realm_run_scheduled(
                statuses.len() as u64,
                2,
                0,
                Some(scheduled_fixture_begin),
                Some(scheduled_fixture_advance),
                Some(scheduled_fixture_finish),
                (&raw const lanes).cast_mut().cast(),
                statuses.as_mut_ptr(),
            )
        },
        0
    );
    assert!(statuses.iter().all(|status| *status == 0));
    let lanes = lanes.lock().unwrap();
    assert_eq!(lanes.len(), 200);
    assert_eq!(
        lanes
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        2
    );
}

struct MigratingFixture {
    realm: u64,
    turns: usize,
    seen: std::sync::Arc<std::sync::Mutex<Vec<(u64, std::thread::ThreadId)>>>,
}

unsafe extern "C" fn migrating_fixture_begin(context: *mut c_void, _index: u64) -> *mut c_void {
    let seen = unsafe {
        &*context.cast::<std::sync::Arc<std::sync::Mutex<Vec<(u64, std::thread::ThreadId)>>>>()
    };
    let mut realm = 0;
    if unsafe { kiln_realm_create(&mut realm) } != 0 || kiln_realm_use_wall_clock(realm) != 0 {
        return ptr::null_mut();
    }
    Box::into_raw(Box::new(MigratingFixture {
        realm,
        turns: 0,
        seen: std::sync::Arc::clone(seen),
    }))
    .cast()
}

unsafe extern "C" fn migrating_fixture_identify(session: *mut c_void) -> u64 {
    unsafe { &*session.cast::<MigratingFixture>() }.realm
}

unsafe extern "C" fn migrating_fixture_advance(session: *mut c_void, _delay: *mut u64) -> i32 {
    unsafe extern "C" fn enter(_: *mut c_void) -> i32 {
        0
    }
    let session = unsafe { &mut *session.cast::<MigratingFixture>() };
    if unsafe { kiln_realm_enter(session.realm, Some(enter), ptr::null_mut()) } != 0 {
        return -1;
    }
    session
        .seen
        .lock()
        .unwrap()
        .push((session.realm, std::thread::current().id()));
    session.turns += 1;
    if session.turns == 3 { 0 } else { 1 }
}

unsafe extern "C" fn migrating_fixture_finish(session: *mut c_void) -> i32 {
    let session = unsafe { Box::from_raw(session.cast::<MigratingFixture>()) };
    kiln_realm_destroy(session.realm)
}

#[test]
fn provider_runs_pure_realm_turns_to_completion_on_shared_lanes() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(
        Vec::<(u64, std::thread::ThreadId)>::new(),
    ));
    let mut statuses = vec![-1; 100];
    assert_eq!(
        unsafe {
            kiln_realm_run_migrating(
                statuses.len() as u64,
                2,
                0,
                Some(migrating_fixture_begin),
                Some(migrating_fixture_identify),
                Some(migrating_fixture_advance),
                Some(migrating_fixture_finish),
                (&raw const seen).cast_mut().cast(),
                statuses.as_mut_ptr(),
            )
        },
        0
    );
    assert!(statuses.iter().all(|status| *status == 0));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), statuses.len() * 3);
    let mut turns_by_realm = HashMap::<u64, usize>::new();
    for &(realm, _) in seen.iter() {
        *turns_by_realm.entry(realm).or_default() += 1;
    }
    assert_eq!(turns_by_realm.len(), statuses.len());
    assert!(turns_by_realm.values().all(|turns| *turns == 3));
}

struct ClaimFixtureState {
    realms: std::sync::Mutex<Vec<u64>>,
    advances: AtomicUsize,
}

struct ClaimFixture {
    realm: u64,
    turns: usize,
    state: Arc<ClaimFixtureState>,
}

unsafe extern "C" fn claim_fixture_begin(context: *mut c_void, _index: u64) -> *mut c_void {
    let state = unsafe { &*context.cast::<Arc<ClaimFixtureState>>() };
    let mut realm = 0;
    if unsafe { kiln_realm_create(&raw mut realm) } != 0 || kiln_realm_use_wall_clock(realm) != 0 {
        return ptr::null_mut();
    }
    state.realms.lock().unwrap().push(realm);
    Box::into_raw(Box::new(ClaimFixture {
        realm,
        turns: 0,
        state: Arc::clone(state),
    }))
    .cast()
}

unsafe extern "C" fn claim_fixture_identify(session: *mut c_void) -> u64 {
    unsafe { &*session.cast::<ClaimFixture>() }.realm
}

unsafe extern "C" fn claim_fixture_advance(session: *mut c_void, delay: *mut u64) -> i32 {
    let session = unsafe { &mut *session.cast::<ClaimFixture>() };
    session.state.advances.fetch_add(1, AtomicOrdering::SeqCst);
    session.turns += 1;
    if session.turns == 1 {
        // A long deadline keeps each body parked until the pair claim wakes it. The deadline
        // still lets the launcher finish if an assertion fails before the claim is made.
        unsafe { delay.write(2_000_000_000) };
        3
    } else {
        0
    }
}

unsafe extern "C" fn claim_fixture_finish(session: *mut c_void) -> i32 {
    let session = unsafe { Box::from_raw(session.cast::<ClaimFixture>()) };
    kiln_realm_destroy(session.realm)
}

#[test]
fn provider_maps_realm_handles_to_pair_claims_without_cross_lane_realm_borrows() {
    // Shift provider IDs independently of this scheduler's ticket IDs. The graph coordinator
    // must route controls through the registered pause targets, never the provider IDs.
    let mut preceding_realm = 0;
    assert_eq!(unsafe { kiln_realm_create(&raw mut preceding_realm) }, 0);
    assert_eq!(kiln_realm_destroy(preceding_realm), 0);
    let state = Arc::new(ClaimFixtureState {
        realms: std::sync::Mutex::new(Vec::new()),
        advances: AtomicUsize::new(0),
    });
    let run_state = Arc::clone(&state);
    let launcher = std::thread::spawn(move || {
        let mut statuses = [-1; 2];
        let result = unsafe {
            kiln_realm_run_migrating(
                2,
                1,
                1,
                Some(claim_fixture_begin),
                Some(claim_fixture_identify),
                Some(claim_fixture_advance),
                Some(claim_fixture_finish),
                (&raw const run_state).cast_mut().cast(),
                statuses.as_mut_ptr(),
            )
        };
        (result, statuses)
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    let realms = loop {
        let realms = state.realms.lock().unwrap().clone();
        if realms.len() == 2 {
            break realms;
        }
        assert!(
            Instant::now() < deadline,
            "both provider realms should register"
        );
        std::thread::yield_now();
    };
    while state.advances.load(AtomicOrdering::SeqCst) < 2 {
        assert!(Instant::now() < deadline, "both first turns should park");
        std::thread::yield_now();
    }
    let claim = claim_scheduled_realm_pair(realms[1], realms[0]).expect("live realm mapping");
    assert!(claim.wait(Duration::from_secs(1)));
    assert_ne!(realms[1], claim.sender.id());
    assert_ne!(realms[0], claim.receiver.id());
    let command = PausedTurnCommand {
        kind: 7,
        ids: [0; 3],
    };
    let sender_control = claim
        .enqueue_sender(command, None)
        .expect("sender target is claimed");
    let receiver_control = claim
        .enqueue_receiver(command, None)
        .expect("receiver target is claimed");
    let _ = sender_control.wait(Duration::from_secs(1));
    let _ = receiver_control.wait(Duration::from_secs(1));
    assert_eq!(state.advances.load(AtomicOrdering::SeqCst), 2);
    assert_eq!(
        claim_scheduled_realm_pair(realms[0], realms[1]).err(),
        Some(ScheduledRealmClaimError::Scheduler(TurnClaimError::Busy))
    );
    assert_eq!(
        claim_scheduled_realm_pair(0, realms[1]).err(),
        Some(ScheduledRealmClaimError::UnknownRealm)
    );
    drop(claim);
    assert_eq!(launcher.join().unwrap(), (0, [0, 0]));
    assert_eq!(state.advances.load(AtomicOrdering::SeqCst), 4);
    assert_eq!(
        claim_scheduled_realm_pair(realms[0], realms[1]).err(),
        Some(ScheduledRealmClaimError::UnknownRealm)
    );
}

struct GraphCoordinatorFixture {
    realms: std::sync::Mutex<[u64; 2]>,
    paused_refusals: AtomicUsize,
}

struct GraphCoordinatorSession {
    state: Arc<GraphCoordinatorFixture>,
    realm: u64,
    index: usize,
    turns: usize,
}

unsafe extern "C" fn graph_coordinator_begin(context: *mut c_void, index: u64) -> *mut c_void {
    let state = unsafe { &*context.cast::<Arc<GraphCoordinatorFixture>>() };
    let mut realm = 0;
    if unsafe { kiln_realm_create(&raw mut realm) } != 0 || kiln_realm_use_wall_clock(realm) != 0 {
        return ptr::null_mut();
    }
    state.realms.lock().unwrap()[index as usize] = realm;
    Box::into_raw(Box::new(GraphCoordinatorSession {
        state: Arc::clone(state),
        realm,
        index: index as usize,
        turns: 0,
    }))
    .cast()
}

unsafe extern "C" fn graph_coordinator_identify(session: *mut c_void) -> u64 {
    unsafe { &*session.cast::<GraphCoordinatorSession>() }.realm
}

unsafe extern "C" fn graph_coordinator_submit(context: *mut c_void) -> i32 {
    let receiver = unsafe { *context.cast::<u64>() };
    i32::from(!submit_graph_claim_request(receiver, 41, 0, 0))
}

unsafe extern "C" fn graph_coordinator_poll(context: *mut c_void) -> i32 {
    let observed = unsafe { &mut *context.cast::<u8>() };
    *observed = graph_claim_request_status(41);
    graph_claim_request_remove(41);
    0
}

unsafe extern "C" fn graph_coordinator_advance(session: *mut c_void, delay: *mut u64) -> i32 {
    let session = unsafe { &mut *session.cast::<GraphCoordinatorSession>() };
    let realms = *session.state.realms.lock().unwrap();
    if realms.contains(&0) {
        return 1;
    }
    if session.turns == 0 {
        if session.index == 0
            && unsafe {
                kiln_realm_enter(
                    session.realm,
                    Some(graph_coordinator_submit),
                    (&raw const realms[1]).cast_mut().cast(),
                )
            } != 0
        {
            return -1;
        }
        session.turns = 1;
        unsafe { delay.write(2_000_000_000) };
        return 3;
    }
    if session.index == 0 {
        let mut observed = 0;
        if unsafe {
            kiln_realm_enter(
                session.realm,
                Some(graph_coordinator_poll),
                (&raw mut observed).cast(),
            )
        } != 0
            || observed != GRAPH_REQUEST_REFUSED_AFTER_PAUSE
        {
            return -1;
        }
        session
            .state
            .paused_refusals
            .fetch_add(1, AtomicOrdering::SeqCst);
    }
    0
}

unsafe extern "C" fn graph_coordinator_finish(session: *mut c_void) -> i32 {
    let session = unsafe { Box::from_raw(session.cast::<GraphCoordinatorSession>()) };
    kiln_realm_destroy(session.realm)
}

#[test]
fn opaque_graph_request_claims_both_realms_then_refuses_without_publication() {
    let state = Arc::new(GraphCoordinatorFixture {
        realms: std::sync::Mutex::new([0; 2]),
        paused_refusals: AtomicUsize::new(0),
    });
    let mut statuses = [-1; 2];
    assert_eq!(
        unsafe {
            kiln_realm_run_migrating(
                2,
                2,
                1,
                Some(graph_coordinator_begin),
                Some(graph_coordinator_identify),
                Some(graph_coordinator_advance),
                Some(graph_coordinator_finish),
                (&raw const state).cast_mut().cast(),
                statuses.as_mut_ptr(),
            )
        },
        0
    );
    assert_eq!(statuses, [0, 0]);
    assert_eq!(state.paused_refusals.load(AtomicOrdering::SeqCst), 1);
}

#[cfg(feature = "ffi")]
#[test]
fn ffi_library_open_pins_subsequent_turns_to_its_os_lane() {
    #[cfg(target_os = "linux")]
    const LIBRARY: &[u8] = b"libc.so.6";
    #[cfg(target_os = "macos")]
    const LIBRARY: &[u8] = b"/usr/lib/libSystem.B.dylib";

    struct Session {
        realm: u64,
        turns: usize,
        lanes: std::sync::Arc<std::sync::Mutex<Vec<std::thread::ThreadId>>>,
    }

    unsafe extern "C" fn begin(context: *mut c_void, _index: u64) -> *mut c_void {
        let lanes = unsafe {
            &*context.cast::<std::sync::Arc<std::sync::Mutex<Vec<std::thread::ThreadId>>>>()
        };
        let mut realm = 0;
        if unsafe { kiln_realm_create(&raw mut realm) } != 0
            || kiln_realm_use_wall_clock(realm) != 0
        {
            return ptr::null_mut();
        }
        Box::into_raw(Box::new(Session {
            realm,
            turns: 0,
            lanes: std::sync::Arc::clone(lanes),
        }))
        .cast()
    }

    unsafe extern "C" fn identify(session: *mut c_void) -> u64 {
        unsafe { &*session.cast::<Session>() }.realm
    }

    unsafe extern "C" fn open_library(context: *mut c_void) -> i32 {
        let library = unsafe { &mut *context.cast::<u64>() };
        unsafe { crate::ffi::kiln_ffi_open(LIBRARY.as_ptr(), LIBRARY.len() as u64, library) }
    }

    unsafe extern "C" fn advance(session: *mut c_void, _delay: *mut u64) -> i32 {
        let session = unsafe { &mut *session.cast::<Session>() };
        if session.turns == 0 {
            let mut library: u64 = 0;
            if unsafe {
                kiln_realm_enter(session.realm, Some(open_library), (&raw mut library).cast())
            } != 0
                || library == 0
                || realm(session.realm).is_none_or(|realm| realm.affinity.get() & AFFINITY_FFI == 0)
            {
                return -1;
            }
        }
        session
            .lanes
            .lock()
            .unwrap()
            .push(std::thread::current().id());
        session.turns += 1;
        if session.turns == 3 { 0 } else { 1 }
    }

    unsafe extern "C" fn finish(session: *mut c_void) -> i32 {
        let session = unsafe { Box::from_raw(session.cast::<Session>()) };
        kiln_realm_destroy(session.realm)
    }

    let lanes = std::sync::Arc::new(std::sync::Mutex::new(Vec::<std::thread::ThreadId>::new()));
    let mut status = -1;
    assert_eq!(
        unsafe {
            kiln_realm_run_migrating(
                1,
                2,
                0,
                Some(begin),
                Some(identify),
                Some(advance),
                Some(finish),
                (&raw const lanes).cast_mut().cast(),
                &raw mut status,
            )
        },
        0
    );
    assert_eq!(status, 0);
    let lanes = lanes.lock().unwrap();
    assert_eq!(lanes.len(), 3);
    assert!(lanes.iter().all(|lane| *lane == lanes[0]));
}

unsafe extern "C" fn exercises_realm_value_slots(context: *mut c_void) -> i32 {
    let slots = unsafe { &mut *context.cast::<[*mut c_void; 3]>() };
    if unsafe { kiln_realm_value_slot(1, &raw mut slots[0]) } != 0
        || unsafe { kiln_realm_value_slot(1, &raw mut slots[1]) } != 0
        || unsafe { kiln_realm_value_slot(2, &raw mut slots[2]) } != 0
    {
        return RUNTIME_ERROR;
    }
    unsafe { slots[0].cast::<u64>().write(37) };
    i32::from(unsafe { slots[1].cast::<u64>().read() } != 37)
}

#[test]
fn opaque_value_slots_are_stable_distinct_and_realm_owned() {
    let mut first_realm = 0;
    let mut second_realm = 0;
    let mut first: [*mut c_void; 3] = [ptr::null_mut(); 3];
    let mut second: [*mut c_void; 3] = [ptr::null_mut(); 3];
    assert_eq!(unsafe { kiln_realm_create(&raw mut first_realm) }, 0);
    assert_eq!(unsafe { kiln_realm_create(&raw mut second_realm) }, 0);
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                first_realm,
                Some(exercises_realm_value_slots),
                (&raw mut first).cast(),
            )
        },
        0
    );
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                second_realm,
                Some(exercises_realm_value_slots),
                (&raw mut second).cast(),
            )
        },
        0
    );
    assert_eq!(first[0], first[1]);
    assert_ne!(first[0], first[2]);
    assert_ne!(first[0], second[0]);
    assert_eq!(
        unsafe { kiln_realm_value_slot(1, ptr::null_mut()) },
        INVALID_ARGUMENT
    );
    assert_eq!(kiln_realm_destroy(first_realm), 0);
    assert_eq!(kiln_realm_destroy(second_realm), 0);
}

unsafe extern "C" fn reads_realm_argv(context: *mut c_void) -> i32 {
    let observed = unsafe { &mut *context.cast::<Vec<Vec<u8>>>() };
    let mut count = 0_u64;
    if unsafe { kiln_realm_argv_count(&raw mut count) } != 0 {
        return RUNTIME_ERROR;
    }
    for index in 0..count {
        let mut bytes = ptr::null();
        let mut len = 0_u64;
        if unsafe { kiln_realm_argv_at(index, &raw mut bytes, &raw mut len) } != 0 {
            return RUNTIME_ERROR;
        }
        observed.push(unsafe { std::slice::from_raw_parts(bytes, len as usize) }.to_vec());
    }
    // One past the end is a question with no answer, not an empty one.
    let mut bytes = ptr::null();
    let mut len = 0_u64;
    i32::from(
        unsafe { kiln_realm_argv_at(count, &raw mut bytes, &raw mut len) } != INVALID_ARGUMENT,
    )
}

/// The launcher's `argv` is borrowed C memory owned by the OS. Copying it is what lets a
/// program keep reading `process.argv` after `main`'s frame has moved on, and what lets
/// this test hand over a buffer it then drops before the realm is ever entered.
#[test]
fn realm_argv_is_copied_from_the_launcher_and_read_back_in_order() {
    let mut realm = 0;
    assert_eq!(unsafe { kiln_realm_create(&raw mut realm) }, 0);
    {
        let owned: Vec<Vec<u8>> = [b"/tmp/program".as_slice(), b"--flag", b""]
            .into_iter()
            .map(|entry| {
                let mut terminated = entry.to_vec();
                terminated.push(0);
                terminated
            })
            .collect();
        let pointers: Vec<*const c_char> = owned
            .iter()
            .map(|entry| entry.as_ptr().cast::<c_char>())
            .collect();
        assert_eq!(
            unsafe { kiln_realm_set_argv(realm, 3, pointers.as_ptr()) },
            0
        );
    }
    let mut observed: Vec<Vec<u8>> = Vec::new();
    assert_eq!(
        unsafe { kiln_realm_enter(realm, Some(reads_realm_argv), (&raw mut observed).cast()) },
        0
    );
    assert_eq!(
        observed,
        vec![b"/tmp/program".to_vec(), b"--flag".to_vec(), Vec::new()]
    );
    // A realm nobody gave a command line reports none rather than guessing at one.
    let mut bare = 0;
    let mut none: Vec<Vec<u8>> = Vec::new();
    assert_eq!(unsafe { kiln_realm_create(&raw mut bare) }, 0);
    assert_eq!(
        unsafe { kiln_realm_enter(bare, Some(reads_realm_argv), (&raw mut none).cast()) },
        0
    );
    assert!(none.is_empty());
    assert_eq!(
        unsafe { kiln_realm_set_argv(0, 0, ptr::null()) },
        INVALID_ARGUMENT
    );
    assert_eq!(kiln_realm_destroy(realm), 0);
    assert_eq!(kiln_realm_destroy(bare), 0);
}

unsafe extern "C" fn draws_realm_entropy(context: *mut c_void) -> i32 {
    let words = unsafe { &mut *context.cast::<[u64; 2]>() };
    if unsafe { kiln_realm_entropy_word(7, &raw mut words[0]) } != 0
        || unsafe { kiln_realm_entropy_word(7, &raw mut words[1]) } != 0
    {
        return RUNTIME_ERROR;
    }
    let mut rejected = 0;
    i32::from(
        unsafe { kiln_realm_entropy_word(8, &raw mut rejected) } != INVALID_ARGUMENT
            || rejected != 0,
    )
}

#[test]
fn deterministic_entropy_is_one_realm_stream_and_rejects_policy_drift() {
    let mut first_realm = 0;
    let mut second_realm = 0;
    let mut first: [u64; 2] = [0; 2];
    let mut second: [u64; 2] = [0; 2];
    assert_eq!(unsafe { kiln_realm_create(&raw mut first_realm) }, 0);
    assert_eq!(unsafe { kiln_realm_create(&raw mut second_realm) }, 0);
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                first_realm,
                Some(draws_realm_entropy),
                (&raw mut first).cast(),
            )
        },
        0
    );
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                second_realm,
                Some(draws_realm_entropy),
                (&raw mut second).cast(),
            )
        },
        0
    );
    assert_ne!(first[0], first[1]);
    assert_eq!(first, second);
    assert_eq!(
        unsafe { kiln_realm_entropy_word(7, ptr::null_mut()) },
        INVALID_ARGUMENT
    );
    assert_eq!(kiln_realm_destroy(first_realm), 0);
    assert_eq!(kiln_realm_destroy(second_realm), 0);
}

unsafe extern "C" fn records_and_renders_diagnostic(context: *mut c_void) -> i32 {
    let site = b"node_modules/provider/index.js";
    let operation = b"CallMethod(listen)";
    let detail = b"unsupported receiver";
    if kiln_realm_diagnostic_reset() != 0
        || unsafe {
            kiln_realm_diagnostic_note(
                site.as_ptr(),
                site.len() as u64,
                37,
                operation.as_ptr(),
                operation.len() as u64,
                3,
                detail.as_ptr(),
                detail.len() as u64,
                ptr::null(),
                0,
                0,
                19,
                0,
            )
        } != 0
    {
        return RUNTIME_ERROR;
    }
    let mut needed = 0;
    if unsafe { kiln_realm_diagnostic_render(ptr::null_mut(), 0, &raw mut needed) }
        != INVALID_ARGUMENT
        || needed == 0
    {
        return RUNTIME_ERROR;
    }
    let buffer = unsafe { &mut *context.cast::<Vec<u8>>() };
    buffer.resize(needed as usize, 0);
    unsafe { kiln_realm_diagnostic_render(buffer.as_mut_ptr(), needed, &raw mut needed) }
}

#[test]
fn diagnostics_are_owned_and_rendered_by_the_active_realm() {
    let mut handle = 0;
    let mut rendered = Vec::new();
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                handle,
                Some(records_and_renders_diagnostic),
                (&raw mut rendered).cast(),
            )
        },
        0
    );
    assert_eq!(
                String::from_utf8(rendered),
                Ok("module=node_modules/provider/index.js instruction=37 operation=CallMethod(listen) detail=unsupported receiver receiver_tag=19".to_owned())
            );
    assert_eq!(kiln_realm_diagnostic_reset(), INVALID_ARGUMENT);
    assert_eq!(kiln_realm_destroy(handle), 0);
}

#[cfg(feature = "realm-http")]
unsafe extern "C" fn close_unregistered_http_connection(context: *mut c_void) -> i32 {
    // A missing HTTP registration is a failed capability lookup, never permission to
    // reinterpret this protocol socket as a raw node:net resource.
    unsafe { *context.cast::<i32>() = kiln_realm_http_server_close_active(42, HTTP_CLOSE_ABORT) };
    0
}

#[test]
#[cfg(feature = "realm-http")]
fn missing_http_registration_never_falls_back_to_raw_close() {
    let mut handle = 0;
    let mut observed = 0;
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                handle,
                Some(close_unregistered_http_connection),
                (&raw mut observed).cast(),
            )
        },
        0
    );
    assert_eq!(observed, INVALID_ARGUMENT);
    assert_eq!(kiln_realm_destroy(handle), 0);
}

#[cfg(feature = "realm-http")]
unsafe extern "C" fn attempt_http_attachment(context: *mut c_void) -> i32 {
    // The resource has no transport. This exercises the public attachment boundary without
    // opening a socket: it must pin before trying to hand a transport to Hyper.
    unsafe {
        *context.cast::<i32>() = kiln_realm_http_server_attach_active(
            42,
            Some(ignore_http_event),
            Some(retain_test_http_owner),
            context,
        );
    }
    0
}

#[test]
#[cfg(feature = "realm-http")]
fn http_attachment_pins_realm_before_dispatch() {
    let reactor = kiln_runtime::Reactor::shared_wall().expect("shared reactor");
    let _scheduled = ScheduledReactorGuard::enter(reactor);
    let mut handle = 0;
    let mut attachment = 0;
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    assert_eq!(kiln_realm_use_wall_clock(handle), 0);
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                handle,
                Some(attempt_http_attachment),
                (&raw mut attachment).cast(),
            )
        },
        0
    );
    assert_eq!(attachment, INVALID_ARGUMENT, "resource has no transport");
    assert_ne!(realm(handle).unwrap().affinity.get() & AFFINITY_HTTP, 0);
    assert!(
        detach_realm(handle).is_none(),
        "a failed attachment conservatively keeps its realm on the original OS lane"
    );
    assert_eq!(kiln_realm_destroy(handle), 0);
}

unsafe extern "C" fn exercises_symbol_registry(context: *mut c_void) -> i32 {
    let output = unsafe { &mut *context.cast::<[u64; 3]>() };
    let key = b"fastify-error-generic";
    let view = crate::KilnString {
        unit_len: key.len() as u32,
        flags: 0,
        data: key.as_ptr().cast(),
    };
    let mut first = 0;
    let mut second = 0;
    let mut borrowed = crate::KilnString {
        unit_len: 0,
        flags: 0,
        data: std::ptr::null(),
    };
    if unsafe { kiln_realm_symbol_for(&view, &raw mut first) } != 0
        || unsafe { kiln_realm_symbol_for(&view, &raw mut second) } != 0
        || first != second
        || unsafe { kiln_realm_symbol_key_for(first, &raw mut borrowed) } != 0
    {
        return RUNTIME_ERROR;
    }
    output[0] = first;
    output[1] = second;
    output[2] = borrowed.unit_len as u64;
    0
}

unsafe extern "C" fn exercises_lossless_symbol_registry(context: *mut c_void) -> i32 {
    let _ = context;
    let high = [0xd800_u16];
    let replacement = [0xfffd_u16];
    let ascii = b"same";
    let ascii_wide = [b's' as u16, b'a' as u16, b'm' as u16, b'e' as u16];
    let high_view = crate::KilnString {
        unit_len: 1,
        flags: 1,
        data: high.as_ptr().cast(),
    };
    let replacement_view = crate::KilnString {
        unit_len: 1,
        flags: 1,
        data: replacement.as_ptr().cast(),
    };
    let ascii_view = crate::KilnString {
        unit_len: ascii.len() as u32,
        flags: 0,
        data: ascii.as_ptr().cast(),
    };
    let ascii_wide_view = crate::KilnString {
        unit_len: ascii_wide.len() as u32,
        flags: 1,
        data: ascii_wide.as_ptr().cast(),
    };
    let mut high_id = 0;
    let mut replacement_id = 0;
    let mut ascii_id = 0;
    let mut ascii_wide_id = 0;
    let mut returned = crate::KilnString {
        unit_len: 0,
        flags: 0,
        data: std::ptr::null(),
    };
    if unsafe { kiln_realm_symbol_for(&high_view, &raw mut high_id) } != 0
        || unsafe { kiln_realm_symbol_for(&replacement_view, &raw mut replacement_id) } != 0
        || unsafe { kiln_realm_symbol_for(&ascii_view, &raw mut ascii_id) } != 0
        || unsafe { kiln_realm_symbol_for(&ascii_wide_view, &raw mut ascii_wide_id) } != 0
        || high_id == replacement_id
        || ascii_id != ascii_wide_id
        || unsafe { kiln_realm_symbol_key_for(high_id, &raw mut returned) } != 0
        || returned.data.is_null()
        || returned.unit_len != 1
        || returned.flags != 1
        || unsafe { returned.data.cast::<u16>().read() } != 0xd800
    {
        return RUNTIME_ERROR;
    }
    0
}

#[test]
fn registered_symbols_are_realm_owned_and_stable() {
    let mut first_realm = 0;
    let mut second_realm = 0;
    let mut first = [0; 3];
    let mut second = [0; 3];
    assert_eq!(unsafe { kiln_realm_create(&raw mut first_realm) }, 0);
    assert_eq!(unsafe { kiln_realm_create(&raw mut second_realm) }, 0);
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                first_realm,
                Some(exercises_symbol_registry),
                (&raw mut first).cast(),
            )
        },
        0
    );
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                second_realm,
                Some(exercises_symbol_registry),
                (&raw mut second).cast(),
            )
        },
        0
    );
    assert_eq!(first[0], first[1]);
    assert_eq!(second[0], second[1]);
    assert_ne!(first[0], second[0]);
    assert_eq!(first[2], b"fastify-error-generic".len() as u64);
    assert_eq!(second[2], b"fastify-error-generic".len() as u64);
    assert_eq!(kiln_realm_destroy(first_realm), 0);
    assert_eq!(kiln_realm_destroy(second_realm), 0);
}

#[test]
fn registered_symbols_preserve_units_and_compare_widths_by_value() {
    let mut realm = 0;
    assert_eq!(unsafe { kiln_realm_create(&raw mut realm) }, 0);
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                realm,
                Some(exercises_lossless_symbol_registry),
                std::ptr::null_mut(),
            )
        },
        0
    );
    assert_eq!(kiln_realm_destroy(realm), 0);
}

unsafe extern "C" fn exercises_callable_store_registry(context: *mut c_void) -> i32 {
    let output = unsafe { &mut *context.cast::<[*mut c_void; 3]>() };
    let first_candidate = opaque_test_pointer(1);
    let duplicate_candidate = opaque_test_pointer(2);
    let other_candidate = opaque_test_pointer(3);
    if unsafe { kiln_realm_callable_store(8, 27, ptr::null_mut(), None, &raw mut output[0]) } != 0
        || !output[0].is_null()
        || unsafe {
            kiln_realm_callable_store(
                8,
                27,
                first_candidate,
                Some(record_callable_store_drop),
                &raw mut output[0],
            )
        } != 0
        || unsafe {
            kiln_realm_callable_store(
                8,
                27,
                duplicate_candidate,
                Some(record_callable_store_drop),
                &raw mut output[1],
            )
        } != 0
        || unsafe {
            kiln_realm_callable_store(
                11,
                27,
                other_candidate,
                Some(record_callable_store_drop),
                &raw mut output[2],
            )
        } != 0
    {
        return RUNTIME_ERROR;
    }
    0
}

unsafe extern "C" fn publishes_second_realm_callable_store(context: *mut c_void) -> i32 {
    let output = unsafe { &mut *context.cast::<*mut c_void>() };
    unsafe {
        kiln_realm_callable_store(
            8,
            27,
            opaque_test_pointer(4),
            Some(record_callable_store_drop),
            output,
        )
    }
}

#[test]
fn runtime_callable_stores_are_stable_distinct_and_realm_owned() {
    CALLABLE_STORE_DROPS.store(0, AtomicOrdering::SeqCst);
    let mut first_realm = 0;
    let mut second_realm = 0;
    let mut first = [ptr::null_mut(); 3];
    let mut second: *mut c_void = ptr::null_mut();
    assert_eq!(unsafe { kiln_realm_create(&raw mut first_realm) }, 0);
    assert_eq!(unsafe { kiln_realm_create(&raw mut second_realm) }, 0);
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                first_realm,
                Some(exercises_callable_store_registry),
                (&raw mut first).cast(),
            )
        },
        0
    );
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                second_realm,
                Some(publishes_second_realm_callable_store),
                (&raw mut second).cast(),
            )
        },
        0
    );
    assert_eq!(first[0], opaque_test_pointer(1));
    assert_eq!(first[1], first[0]);
    assert_eq!(first[2], opaque_test_pointer(3));
    assert_eq!(second, opaque_test_pointer(4));
    assert_eq!(
        unsafe { kiln_realm_callable_store(8, 27, ptr::null_mut(), None, ptr::null_mut()) },
        INVALID_ARGUMENT
    );
    assert_eq!(kiln_realm_destroy(first_realm), 0);
    assert_eq!(kiln_realm_destroy(second_realm), 0);
    assert_eq!(CALLABLE_STORE_DROPS.load(AtomicOrdering::SeqCst), 3);
}

#[test]
fn the_collector_seam_marks_registered_roots_and_tears_down_once() {
    use std::sync::atomic::AtomicUsize;
    static COLLECTS: AtomicUsize = AtomicUsize::new(0);
    static BEGINS: AtomicUsize = AtomicUsize::new(0);
    static ENDS: AtomicUsize = AtomicUsize::new(0);
    static MARKED_VALUES: AtomicUsize = AtomicUsize::new(0);
    static ROOT_PHASES: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());

    unsafe extern "C" fn collect(_heap: *mut c_void, _force: i32) {
        COLLECTS.fetch_add(1, AtomicOrdering::SeqCst);
    }
    unsafe extern "C" fn teardown_begin(_heap: *mut c_void) {
        BEGINS.fetch_add(1, AtomicOrdering::SeqCst);
    }
    unsafe extern "C" fn teardown_end(heap: *mut c_void) {
        ENDS.fetch_add(1, AtomicOrdering::SeqCst);
        // SAFETY: the test allocated this heap as one Box below.
        drop(unsafe { Box::from_raw(heap.cast::<[*const GcHooks; 2]>()) });
    }
    unsafe extern "C" fn mark_value(_heap: *mut c_void, _value: *const c_void) {
        MARKED_VALUES.fetch_add(1, AtomicOrdering::SeqCst);
    }
    unsafe extern "C" fn mark_block(_heap: *mut c_void, _pointer: *mut c_void) {}
    unsafe extern "C" fn introspect(_heap: *mut c_void) -> i32 {
        0
    }
    unsafe extern "C" fn roots(heap: *mut c_void, context: *mut c_void, phase: i32) {
        ROOT_PHASES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(phase);
        if phase == GC_PHASE_MARK || phase == GC_PHASE_TEARDOWN {
            assert!(!heap.is_null());
            // SAFETY: the heap is live in both marking phases.
            assert_eq!(unsafe { kiln_realm_gc_mark_value(heap, context) }, 0);
        }
    }
    static HOOKS: GcHooks = GcHooks {
        version: GC_HOOKS_VERSION,
        size: std::mem::size_of::<GcHooks>() as u32,
        collect: Some(collect),
        teardown_begin: Some(teardown_begin),
        teardown_end: Some(teardown_end),
        mark_value: Some(mark_value),
        mark_block: Some(mark_block),
        introspect: Some(introspect),
    };

    unsafe extern "C" fn enter(context: *mut c_void) -> i32 {
        let mut heap = std::ptr::null_mut();
        // SAFETY: every pointer handed over below stays live for the call.
        unsafe {
            // No heap yet: attach reports null without an error.
            assert_eq!(
                kiln_realm_gc_attach(&raw const HOOKS, std::ptr::null_mut(), &raw mut heap),
                0
            );
            assert!(heap.is_null());
            let candidate =
                Box::into_raw(Box::new([&raw const HOOKS, std::ptr::null()])).cast::<c_void>();
            assert_eq!(
                kiln_realm_gc_attach(&raw const HOOKS, candidate, &raw mut heap),
                0
            );
            assert_eq!(heap, candidate);
            // A second candidate is refused in favour of the existing heap.
            let other =
                Box::into_raw(Box::new([&raw const HOOKS, std::ptr::null()])).cast::<c_void>();
            assert_eq!(
                kiln_realm_gc_attach(&raw const HOOKS, other, &raw mut heap),
                0
            );
            assert_eq!(heap, candidate);
            drop(Box::from_raw(other.cast::<[*const GcHooks; 2]>()));
            assert_eq!(
                kiln_realm_gc_register_roots(Some(roots), context, GC_ROOT_SURVIVES_TEARDOWN),
                0
            );
            assert_eq!(
                kiln_realm_gc_register_roots(Some(roots), context, GC_ROOT_SURVIVES_TEARDOWN),
                0
            );
            assert_eq!(kiln_realm_gc_mark_roots(heap), 0);
        }
        0
    }

    let mut handle = 0;
    let mut slot = [0u64; 2];
    // SAFETY: the output slot and the root context stay live through the calls.
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    assert_eq!(
        unsafe { kiln_realm_enter(handle, Some(enter), (&raw mut slot).cast::<c_void>()) },
        0
    );
    assert_eq!(
        ROOT_PHASES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_slice(),
        &[GC_PHASE_MARK]
    );
    assert_eq!(MARKED_VALUES.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(kiln_realm_destroy(handle), 0);
    assert_eq!(BEGINS.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(ENDS.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(
        ROOT_PHASES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_slice(),
        &[GC_PHASE_MARK, GC_PHASE_TEARDOWN, GC_PHASE_RESET]
    );
    assert_eq!(MARKED_VALUES.load(AtomicOrdering::SeqCst), 2);
    assert_eq!(COLLECTS.load(AtomicOrdering::SeqCst), 0);
}

#[test]
fn provider_root_visitor_finds_aliases_without_marking_or_invoking_root_callbacks() {
    static MARKS: AtomicUsize = AtomicUsize::new(0);
    static ROOT_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn collect(_heap: *mut c_void, _force: i32) {}
    unsafe extern "C" fn teardown_begin(_heap: *mut c_void) {}
    unsafe extern "C" fn teardown_end(heap: *mut c_void) {
        // SAFETY: enter allocated this two-word fake heap.
        drop(unsafe { Box::from_raw(heap.cast::<[*const GcHooks; 2]>()) });
    }
    unsafe extern "C" fn mark_value(_heap: *mut c_void, _value: *const c_void) {
        MARKS.fetch_add(1, AtomicOrdering::SeqCst);
    }
    unsafe extern "C" fn mark_block(_heap: *mut c_void, _block: *mut c_void) {
        MARKS.fetch_add(1, AtomicOrdering::SeqCst);
    }
    unsafe extern "C" fn introspect(_heap: *mut c_void) -> i32 {
        0
    }
    unsafe extern "C" fn registered_roots(_heap: *mut c_void, _context: *mut c_void, _phase: i32) {
        ROOT_CALLS.fetch_add(1, AtomicOrdering::SeqCst);
    }
    unsafe extern "C" fn paired_roots(_heap: *mut c_void, _context: *mut c_void, _phase: i32) {
        ROOT_CALLS.fetch_add(1, AtomicOrdering::SeqCst);
    }
    unsafe extern "C" fn paired_visit(
        context: *mut c_void,
        visit: GcVisitRoot,
        visit_context: *mut c_void,
    ) -> i32 {
        let probe = unsafe { &*context.cast::<Probe>() };
        // Model an enumerator that ignores rejection. The provider's relay still records it.
        let _ = unsafe {
            visit(
                visit_context,
                probe.paired_only as *const c_void,
                GC_VISIT_BLOCK,
            )
        };
        0
    }
    unsafe extern "C" fn drop_work(_pointer: *mut c_void) {}
    unsafe extern "C" fn run_work(
        _pointer: *mut c_void,
        _host: *mut c_void,
        _thrown: *mut c_void,
    ) -> i32 {
        0
    }
    static HOOKS: GcHooks = GcHooks {
        version: GC_HOOKS_VERSION,
        size: std::mem::size_of::<GcHooks>() as u32,
        collect: Some(collect),
        teardown_begin: Some(teardown_begin),
        teardown_end: Some(teardown_end),
        mark_value: Some(mark_value),
        mark_block: Some(mark_block),
        introspect: Some(introspect),
    };

    #[derive(Default)]
    struct Probe {
        slot: usize,
        candidate: usize,
        work: usize,
        paired_only: usize,
        seen_slot: bool,
        seen_candidate: bool,
        seen_work: bool,
        seen_paired: bool,
    }
    unsafe extern "C" fn visit(context: *mut c_void, pointer: *const c_void, kind: u32) -> i32 {
        let probe = unsafe { &mut *context.cast::<Probe>() };
        let address = pointer as usize;
        if kind == GC_VISIT_VALUE && address == probe.slot {
            probe.seen_slot = true;
        }
        if kind == GC_VISIT_BLOCK && address == probe.candidate {
            probe.seen_candidate = true;
        }
        if kind == GC_VISIT_BLOCK && address == probe.work {
            probe.seen_work = true;
        }
        if kind == GC_VISIT_BLOCK && address == probe.paired_only {
            probe.seen_paired = true;
        }
        0
    }
    unsafe extern "C" fn reject_visit(
        _context: *mut c_void,
        _pointer: *const c_void,
        _kind: u32,
    ) -> i32 {
        1
    }
    unsafe extern "C" fn reject_paired(
        context: *mut c_void,
        pointer: *const c_void,
        kind: u32,
    ) -> i32 {
        let probe = unsafe { &*context.cast::<Probe>() };
        if kind == GC_VISIT_BLOCK && pointer as usize == probe.paired_only {
            1
        } else {
            0
        }
    }
    unsafe extern "C" fn enter(context: *mut c_void) -> i32 {
        let probe = unsafe { &mut *context.cast::<Probe>() };
        let mut heap = ptr::null_mut();
        let candidate_heap =
            Box::into_raw(Box::new([&raw const HOOKS, ptr::null()])).cast::<c_void>();
        assert_eq!(
            unsafe { kiln_realm_gc_attach(&raw const HOOKS, candidate_heap, &raw mut heap) },
            0
        );
        let mut slot = ptr::null_mut();
        assert_eq!(unsafe { kiln_realm_value_slot(928, &raw mut slot) }, 0);
        probe.slot = slot as usize;
        let mut store = ptr::null_mut();
        assert_eq!(
            unsafe {
                kiln_realm_callable_store(
                    1,
                    928,
                    probe.candidate as *mut c_void,
                    Some(drop_work),
                    &raw mut store,
                )
            },
            0
        );
        assert_eq!(store as usize, probe.candidate);
        assert_eq!(
            unsafe {
                kiln_realm_queue_microtask(
                    Some(run_work),
                    Some(drop_work),
                    probe.work as *mut c_void,
                )
            },
            0
        );
        assert_eq!(
            unsafe { kiln_realm_gc_visit_provider_roots(heap, Some(visit), context) },
            0
        );
        assert!(probe.seen_slot && probe.seen_candidate && probe.seen_work);
        assert_eq!(MARKS.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(ROOT_CALLS.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(
            unsafe { kiln_realm_gc_has_root_visitor(heap, Some(paired_visit), context) },
            0
        );
        assert_eq!(
            unsafe {
                kiln_realm_gc_register_roots_with_visitor(
                    Some(paired_roots),
                    Some(paired_visit),
                    context,
                    0,
                )
            },
            0
        );
        assert_eq!(
            unsafe { kiln_realm_gc_visit_provider_roots(heap, Some(visit), context) },
            0
        );
        assert!(probe.seen_paired);
        assert_eq!(
            unsafe { kiln_realm_gc_has_root_visitor(heap, Some(paired_visit), context) },
            1
        );
        assert_eq!(
            unsafe { kiln_realm_gc_has_root_visitor(heap, Some(paired_visit), slot) },
            0
        );
        assert_eq!(
            unsafe { kiln_realm_gc_has_root_visitor(ptr::null_mut(), Some(paired_visit), context) },
            INVALID_ARGUMENT
        );

        assert_eq!(
            unsafe { kiln_realm_gc_visit_provider_roots(heap, Some(reject_paired), context) },
            RUNTIME_ERROR
        );
        assert_eq!(
            unsafe { kiln_realm_gc_unregister_roots(Some(paired_roots), context) },
            0
        );
        assert_eq!(
            unsafe { kiln_realm_gc_has_root_visitor(heap, Some(paired_visit), context) },
            0
        );
        assert_eq!(
            unsafe { kiln_realm_gc_register_roots(Some(registered_roots), context, 0) },
            0
        );
        assert_eq!(
            unsafe { kiln_realm_gc_visit_provider_roots(heap, Some(visit), context) },
            GC_VISIT_REGISTERED_ROOTS_UNENUMERATED
        );
        assert_eq!(
            unsafe { kiln_realm_gc_visit_provider_roots(heap, Some(reject_visit), context) },
            RUNTIME_ERROR
        );
        assert_eq!(MARKS.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(ROOT_CALLS.load(AtomicOrdering::SeqCst), 0);
        0
    }

    let mut handle = 0;
    let mut probe = Probe {
        candidate: opaque_test_pointer(0x9280) as usize,
        work: opaque_test_pointer(0x9281) as usize,
        paired_only: opaque_test_pointer(0x9282) as usize,
        ..Probe::default()
    };
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    assert_eq!(
        unsafe { kiln_realm_enter(handle, Some(enter), (&raw mut probe).cast()) },
        0
    );
    assert_eq!(kiln_realm_destroy(handle), 0);
}

/// Realms issue 24. Teardown runs prelude code -- a callable store's destructor releases its
/// record, the collector's teardown hooks sweep -- and the prelude finds its heap by asking which
/// one is current on the thread. A realm is destroyed outside `ActiveRealm`, so each of those
/// used to find none and fall back to one process-wide heap, which every realm thread tearing
/// down at the same moment initialised and used at once (a segfault in about one run in a
/// thousand at `KILN_REALMS=16`). The realm's own heap must be current for the whole teardown,
/// and nothing may be current once the heap is freed.
#[test]
fn teardown_runs_with_the_realms_own_heap_current_and_none_after() {
    static SEEN: std::sync::Mutex<Vec<(&'static str, usize)>> = std::sync::Mutex::new(Vec::new());
    fn note(what: &'static str) {
        SEEN.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((what, kiln_realm_gc_current_heap() as usize));
    }
    unsafe extern "C" fn collect(_heap: *mut c_void, _force: i32) {}
    unsafe extern "C" fn teardown_begin(_heap: *mut c_void) {
        note("teardown_begin");
    }
    unsafe extern "C" fn teardown_end(heap: *mut c_void) {
        note("teardown_end");
        // SAFETY: the test allocated this heap as one Box below.
        drop(unsafe { Box::from_raw(heap.cast::<[*const GcHooks; 2]>()) });
    }
    unsafe extern "C" fn mark_value(_heap: *mut c_void, _value: *const c_void) {}
    unsafe extern "C" fn mark_block(_heap: *mut c_void, _pointer: *mut c_void) {}
    unsafe extern "C" fn introspect(_heap: *mut c_void) -> i32 {
        0
    }
    unsafe extern "C" fn roots(_heap: *mut c_void, _context: *mut c_void, phase: i32) {
        if phase == GC_PHASE_RESET {
            note("reset");
        }
    }
    unsafe extern "C" fn store_drop(_pointer: *mut c_void) {
        note("callable store");
    }
    static HOOKS: GcHooks = GcHooks {
        version: GC_HOOKS_VERSION,
        size: std::mem::size_of::<GcHooks>() as u32,
        collect: Some(collect),
        teardown_begin: Some(teardown_begin),
        teardown_end: Some(teardown_end),
        mark_value: Some(mark_value),
        mark_block: Some(mark_block),
        introspect: Some(introspect),
    };
    unsafe extern "C" fn enter(context: *mut c_void) -> i32 {
        let mut heap = std::ptr::null_mut();
        let mut store = std::ptr::null_mut();
        let candidate =
            Box::into_raw(Box::new([&raw const HOOKS, std::ptr::null()])).cast::<c_void>();
        // SAFETY: the candidate stays live until teardown frees it; the outputs are this frame's
        // and the caller's.
        unsafe {
            assert_eq!(
                kiln_realm_gc_attach(&raw const HOOKS, candidate, &raw mut heap),
                0
            );
            assert_eq!(kiln_realm_gc_register_roots(Some(roots), context, 0), 0);
            assert_eq!(
                kiln_realm_callable_store(
                    8,
                    27,
                    opaque_test_pointer(5),
                    Some(store_drop),
                    &raw mut store,
                ),
                0
            );
            context.cast::<usize>().write(heap as usize);
        }
        0
    }

    let mut handle = 0;
    let mut heap = 0usize;
    // SAFETY: the output slot and the root context stay live through the calls.
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    assert_eq!(
        unsafe { kiln_realm_enter(handle, Some(enter), (&raw mut heap).cast::<c_void>()) },
        0
    );
    assert_ne!(heap, 0);
    assert!(
        kiln_realm_gc_current_heap().is_null(),
        "no realm is active between entries"
    );
    let generation = unsafe { *kiln_realm_generation_slot() };
    assert_eq!(kiln_realm_destroy(handle), 0);
    assert_eq!(
        SEEN.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_slice(),
        &[
            ("callable store", heap),
            ("teardown_begin", heap),
            ("teardown_end", heap),
            ("reset", 0),
        ]
    );
    assert!(kiln_realm_gc_current_heap().is_null());
    assert_ne!(
        unsafe { *kiln_realm_generation_slot() },
        generation,
        "a cached answer from before the teardown must be asked again"
    );
}

/// The dump has to be offered where the collector sweeps, and the numbers it prints have
/// to be readable from there: the prelude calls back into this archive in the middle of
/// the hook, so anything the drive still had borrowed would panic instead of printing.
#[test]
fn the_turn_boundary_offers_the_dump_and_the_report_reads_the_runtime() -> Result<(), Box<dyn Error>>
{
    use std::sync::atomic::AtomicUsize;
    static OFFERS: AtomicUsize = AtomicUsize::new(0);
    static REPORT: std::sync::Mutex<Option<(i32, u64, [u64; 9])>> = std::sync::Mutex::new(None);

    unsafe extern "C" fn collect(_heap: *mut c_void, _force: i32) {}
    unsafe extern "C" fn teardown_begin(_heap: *mut c_void) {}
    unsafe extern "C" fn teardown_end(heap: *mut c_void) {
        // SAFETY: the test allocated this heap as one Box below.
        drop(unsafe { Box::from_raw(heap.cast::<[*const GcHooks; 2]>()) });
    }
    unsafe extern "C" fn mark_value(_heap: *mut c_void, _value: *const c_void) {}
    unsafe extern "C" fn mark_block(_heap: *mut c_void, _pointer: *mut c_void) {}
    unsafe extern "C" fn introspect(_heap: *mut c_void) -> i32 {
        OFFERS.fetch_add(1, AtomicOrdering::SeqCst);
        let mut fields = [u64::MAX; 9];
        let mut count = u64::MAX;
        // SAFETY: both output spans are this frame's, and the hook runs at the turn
        // boundary with the realm active.
        let status = unsafe { kiln_realm_runtime_report(fields.as_mut_ptr(), 9, &raw mut count) };
        *REPORT.lock().unwrap_or_else(PoisonError::into_inner) = Some((status, count, fields));
        INTROSPECT_ARMED
    }
    static HOOKS: GcHooks = GcHooks {
        version: GC_HOOKS_VERSION,
        size: std::mem::size_of::<GcHooks>() as u32,
        collect: Some(collect),
        teardown_begin: Some(teardown_begin),
        teardown_end: Some(teardown_end),
        mark_value: Some(mark_value),
        mark_block: Some(mark_block),
        introspect: Some(introspect),
    };

    unsafe extern "C" fn enter(_context: *mut c_void) -> i32 {
        let mut heap = std::ptr::null_mut();
        let candidate =
            Box::into_raw(Box::new([&raw const HOOKS, std::ptr::null()])).cast::<c_void>();
        // SAFETY: the candidate stays live until teardown frees it.
        unsafe { kiln_realm_gc_attach(&raw const HOOKS, candidate, &raw mut heap) }
    }

    let mut handle = 0;
    // SAFETY: the output slot stays live through the call.
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    assert_eq!(
        unsafe { kiln_realm_enter(handle, Some(enter), std::ptr::null_mut()) },
        0
    );
    assert_eq!(
        unsafe { kiln_realm_drive(handle, std::ptr::null_mut(), std::ptr::null_mut()) },
        0
    );
    assert_eq!(OFFERS.load(AtomicOrdering::SeqCst), 1);

    let (status, count, fields) = REPORT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .ok_or("the hook reported")?;
    assert_eq!(status, 0);
    assert_eq!(count, RUNTIME_REPORT_FIELDS as u64);
    // A realm runtime owns exactly one worker, and this one served nothing.
    assert_eq!(fields[5], 1, "workers");
    assert_eq!(
        &fields[6..9],
        &[0, 0, 0],
        "connections and requests in flight"
    );
    assert_eq!(kiln_realm_destroy(handle), 0);
    Ok(())
}

/// The park cap exists so an idle server answers a signal; it must not touch a
/// virtual-clock realm, whose park is free by construction and whose determinism would
/// otherwise start depending on real time.
#[test]
fn a_park_is_capped_only_for_an_armed_wall_clock_realm() -> Result<(), Box<dyn Error>> {
    let realm = NativeRealm::new()?;
    let brief = std::time::Duration::from_millis(5);
    let hour = std::time::Duration::from_secs(3600);

    assert_eq!(introspect_park(&realm, None), None);
    realm.introspect_armed.set(true);
    assert_eq!(introspect_park(&realm, None), None, "virtual clock");
    assert_eq!(
        introspect_park(&realm, Some(hour)),
        Some(hour),
        "virtual clock"
    );

    realm.clock.set(NativeClock::Wall {
        epoch: std::time::Duration::ZERO,
        started: std::time::Instant::now(),
    });
    assert_eq!(introspect_park(&realm, None), Some(INTROSPECT_POLL));
    assert_eq!(introspect_park(&realm, Some(hour)), Some(INTROSPECT_POLL));
    assert_eq!(introspect_park(&realm, Some(brief)), Some(brief));

    realm.introspect_armed.set(false);
    assert_eq!(introspect_park(&realm, None), None);
    assert_eq!(introspect_park(&realm, Some(hour)), Some(hour));
    Ok(())
}

struct RecordedWork {
    value: u8,
    order: *mut Vec<u8>,
    drops: *mut usize,
    clear: u64,
}

struct ReadObservation {
    status: i32,
    request: u64,
    sequence: u64,
    bytes: Vec<u8>,
    error_code: Vec<u8>,
    drops: usize,
}

struct ReadSubmission {
    handle: u64,
    path: Vec<u8>,
    observation: *mut ReadObservation,
    request: *mut u64,
}

struct ReadAndTimerSubmission {
    read: ReadSubmission,
    order: *mut Vec<u8>,
    drops: *mut usize,
}

#[derive(Default)]
struct HostObservation {
    status: i32,
    request: u64,
    sequence: u64,
    value_kind: u32,
    bytes: Vec<u8>,
    value_u64: u64,
    resource: u64,
    resource_kind: u32,
    resource_port: u32,
    resource_flags: u32,
    address: Vec<u8>,
    peer_address: Vec<u8>,
    peer_port: u32,
    drops: usize,
}

struct HostWork {
    observation: *mut HostObservation,
}

unsafe extern "C" fn record_host_completion(
    work: *mut c_void,
    completion: *const CompletionView,
    _host: *mut c_void,
    _thrown: *mut c_void,
) -> i32 {
    if work.is_null() || completion.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: this continuation and completion view are owned by the current request.
    let target = unsafe { &mut *(*work.cast::<HostWork>()).observation };
    // SAFETY: the runtime keeps the completion view and all referenced bytes live until
    // this callback returns.
    let completion = unsafe { &*completion };
    target.status = completion.status;
    target.request = completion.request;
    target.sequence = completion.sequence;
    target.value_kind = completion.value_kind;
    target.value_u64 = completion.value_u64;
    target.resource = completion.resource;
    target.resource_kind = completion.resource_kind;
    target.resource_port = completion.resource_port;
    target.resource_flags = completion.resource_flags;
    target.peer_port = completion.peer_port;
    for (pointer, length, destination) in [
        (completion.bytes, completion.bytes_len, &mut target.bytes),
        (
            completion.address,
            completion.address_len,
            &mut target.address,
        ),
        (
            completion.peer_address,
            completion.peer_address_len,
            &mut target.peer_address,
        ),
    ] {
        let length = usize::try_from(length).unwrap_or(usize::MAX);
        if length != 0 && pointer.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: each completion byte span is borrowed for this callback.
        *destination = unsafe { std::slice::from_raw_parts(pointer, length) }.to_vec();
    }
    0
}

unsafe extern "C" fn drop_host_work(work: *mut c_void) {
    if work.is_null() {
        return;
    }
    // SAFETY: the realm invokes the matching disposer exactly once.
    let work = unsafe { Box::from_raw(work.cast::<HostWork>()) };
    // SAFETY: the test observation outlives the request and realm.
    unsafe {
        (*work.observation).drops = (*work.observation).drops.saturating_add(1);
    }
}

unsafe fn start_test_host_op(
    handle: u64,
    operation: &HostOpView,
    observation: *mut HostObservation,
    request: *mut u64,
) -> i32 {
    let work = Box::into_raw(Box::new(HostWork { observation })).cast::<c_void>();
    // SAFETY: every pointer remains live through submission and the continuation targets
    // remain live through realm drive.
    let status = unsafe {
        kiln_realm_start_host_op(
            handle,
            operation,
            Some(record_host_completion),
            Some(drop_host_work),
            work,
            request,
        )
    };
    if status != 0 {
        // SAFETY: ownership did not transfer on failure.
        unsafe { drop_host_work(work) };
    }
    status
}

unsafe fn start_test_active_host_op(
    operation: &HostOpView,
    observation: *mut HostObservation,
    request: *mut u64,
) -> i32 {
    let work = Box::into_raw(Box::new(HostWork { observation })).cast::<c_void>();
    // SAFETY: the active entry copies the frame before returning and owns the continuation
    // only when submission succeeds.
    let status = unsafe {
        kiln_realm_start_active_host_op(
            operation,
            Some(record_host_completion),
            Some(drop_host_work),
            work,
            request,
        )
    };
    if status != 0 {
        // SAFETY: ownership did not transfer on failure.
        unsafe { drop_host_work(work) };
    }
    status
}

#[cfg(feature = "realm-http")]
struct RegisteredHttpCloseAttempt {
    status: i32,
    request: u64,
    observation: HostObservation,
}

#[cfg(feature = "realm-http")]
unsafe extern "C" fn ignore_http_event(
    _work: *mut c_void,
    _event: *const HttpEventView,
    _host: *mut c_void,
    _thrown: *mut c_void,
) -> i32 {
    0
}

#[cfg(feature = "realm-http")]
unsafe extern "C" fn retain_test_http_owner(_work: *mut c_void) {}

#[cfg(feature = "realm-http")]
unsafe extern "C" fn try_raw_close_on_registered_http(context: *mut c_void) -> i32 {
    if context.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the test keeps this state live until after realm destruction.
    let attempt = unsafe { &mut *context.cast::<RegisteredHttpCloseAttempt>() };
    let Some(realm) = active_realm() else {
        return INVALID_ARGUMENT;
    };
    let resource = ResourceId::new(42);
    realm.http_connections.borrow_mut().insert(
        resource,
        NativeHttpConnection {
            role: HttpRole::Server,
            events: Rc::new(HttpEventOwner {
                pointer: context,
                run: ignore_http_event,
                drop: retain_test_http_owner,
            }),
        },
    );
    let operation = HostOpView {
        operation: OP_CLOSE_RESOURCE,
        flags: 0,
        resource: resource.get(),
        argument: 0,
        text: ptr::null(),
        text_len: 0,
        bytes: ptr::null(),
        bytes_len: 0,
    };
    // SAFETY: the operation is copied synchronously and all observation slots stay live.
    attempt.status = unsafe {
        start_test_active_host_op(
            &operation,
            &raw mut attempt.observation,
            &raw mut attempt.request,
        )
    };
    0
}

#[test]
#[cfg(feature = "realm-http")]
fn registered_http_connection_rejects_raw_hostops_close() {
    let mut handle = 0;
    let mut attempt = RegisteredHttpCloseAttempt {
        status: 0,
        request: 0,
        observation: HostObservation::default(),
    };
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                handle,
                Some(try_raw_close_on_registered_http),
                (&raw mut attempt).cast(),
            )
        },
        0
    );
    assert_eq!(attempt.status, INVALID_ARGUMENT);
    assert_eq!(attempt.request, 0);
    assert_eq!(attempt.observation.drops, 1);
    assert!(
        realm(handle).is_some_and(|registered| registered.continuations.borrow().is_empty()),
        "realm remains registered through the test, with no continuations left"
    );
    assert_eq!(kiln_realm_destroy(handle), 0);
}

unsafe extern "C" fn record_read_completion(
    work: *mut c_void,
    completion: *const CompletionView,
    _host: *mut c_void,
    _thrown: *mut c_void,
) -> i32 {
    if work.is_null() || completion.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the request owns this continuation and the completion view for the
    // callback's duration.
    let observation = unsafe { &mut *work.cast::<ReadWork>() }.observation;
    let completion = unsafe { &*completion };
    // SAFETY: completion pointers remain valid until this callback returns.
    let target = unsafe { &mut *observation };
    target.status = completion.status;
    target.request = completion.request;
    target.sequence = completion.sequence;
    if completion.status == 0 {
        let length = usize::try_from(completion.bytes_len).unwrap_or(usize::MAX);
        if length > 0 && completion.bytes.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the runtime owns the bytes for this callback.
        target.bytes = unsafe { std::slice::from_raw_parts(completion.bytes, length) }.to_vec();
    } else {
        let length = usize::try_from(completion.error_code_len).unwrap_or(usize::MAX);
        if length > 0 && completion.error_code.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the runtime owns the error code for this callback.
        target.error_code =
            unsafe { std::slice::from_raw_parts(completion.error_code, length) }.to_vec();
    }
    0
}

struct ReadWork {
    observation: *mut ReadObservation,
}

unsafe extern "C" fn drop_read_work(work: *mut c_void) {
    if work.is_null() {
        return;
    }
    // SAFETY: the continuation is dropped exactly once by the Realm owner.
    let work = unsafe { Box::from_raw(work.cast::<ReadWork>()) };
    // SAFETY: the observation outlives the request and its disposer.
    unsafe { (*work.observation).drops = (*work.observation).drops.saturating_add(1) };
}

unsafe extern "C" fn submit_read(context: *mut c_void) -> i32 {
    if context.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the test owns the submission for the duration of realm entry.
    let submission = unsafe { &mut *context.cast::<ReadSubmission>() };
    let work = Box::new(ReadWork {
        observation: submission.observation,
    });
    let pointer = Box::into_raw(work).cast::<c_void>();
    // SAFETY: all pointers refer to test-owned storage live through the Realm drive.
    unsafe {
        kiln_realm_read_file(
            submission.handle,
            submission.path.as_ptr(),
            u64::try_from(submission.path.len()).unwrap_or(u64::MAX),
            Some(record_read_completion),
            Some(drop_read_work),
            pointer,
            submission.request,
        )
    }
}

unsafe extern "C" fn submit_and_cancel(context: *mut c_void) -> i32 {
    // SAFETY: the caller provides the same live submission used by `submit_read`.
    let status = unsafe { submit_read(context) };
    if status != 0 {
        return status;
    }
    // SAFETY: `context` remains live and contains the request just submitted.
    let submission = unsafe { &mut *context.cast::<ReadSubmission>() };
    unsafe { kiln_realm_cancel_request(submission.handle, *submission.request) }
}

unsafe extern "C" fn submit_read_and_timer(context: *mut c_void) -> i32 {
    if context.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the test owns this combined submission for the duration of realm entry.
    let submission = unsafe { &mut *context.cast::<ReadAndTimerSubmission>() };
    // SAFETY: the nested read submission and every callback target remain live through
    // the subsequent realm drive.
    let status = unsafe { submit_read((&raw mut submission.read).cast::<c_void>()) };
    if status != 0 {
        return status;
    }
    // SAFETY: both target pointers are test-owned until realm teardown.
    let timer = unsafe { recorded_work(9, &mut *submission.order, &mut *submission.drops) };
    let mut id = 0;
    // SAFETY: callbacks match the transferred allocation and the identity slot is live.
    let status = unsafe {
        kiln_realm_set_timeout(
            3_600_000.0,
            Some(record_work),
            Some(drop_recorded_work),
            timer,
            &raw mut id,
        )
    };
    if status != 0 {
        // SAFETY: ownership did not transfer on failure.
        unsafe { drop_recorded_work(timer) };
    }
    status
}

struct CancelSubmission {
    handle: u64,
    request: u64,
    result: *mut i32,
}

unsafe extern "C" fn cancel_request(context: *mut c_void) -> i32 {
    if context.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the test owns this context for the duration of realm entry.
    let submission = unsafe { &mut *context.cast::<CancelSubmission>() };
    let result = unsafe { kiln_realm_cancel_request(submission.handle, submission.request) };
    // SAFETY: the result slot remains live for this callback.
    unsafe { submission.result.write(result) };
    0
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
        if (*work).value == 5 {
            let nested = recorded_work(6, &mut *(*work).order, &mut *(*work).drops);
            let status =
                kiln_realm_queue_next_tick(Some(record_work), Some(drop_recorded_work), nested);
            if status != 0 {
                drop_recorded_work(nested);
                return status;
            }
        }
        if (*work).clear != 0 {
            return kiln_realm_clear_timer((*work).clear);
        }
    }
    0
}

unsafe extern "C" fn fail_recorded_work(
    work: *mut c_void,
    _host: *mut c_void,
    _thrown: *mut c_void,
) -> i32 {
    if work.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the test owns the record until Realm drive drops the popped payload.
    let work = work.cast::<RecordedWork>();
    unsafe {
        (*(*work).order).push((*work).value);
    }
    RUNTIME_ERROR
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
    let next_tick = recorded_work(5, order, drops);
    // SAFETY: callbacks match the transferred allocation.
    let mut status = unsafe {
        kiln_realm_queue_next_tick(Some(record_work), Some(drop_recorded_work), next_tick)
    };
    if status != 0 {
        // SAFETY: ownership did not transfer on failure.
        unsafe { drop_recorded_work(next_tick) };
        return status;
    }
    let microtask = recorded_work(1, order, drops);
    // SAFETY: callbacks match the transferred allocation.
    status = unsafe {
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

unsafe extern "C" fn reads_performance_clock(context: *mut c_void) -> i32 {
    // SAFETY: the test supplies one live `f64` slot while this callback runs.
    unsafe { kiln_realm_performance_now(context.cast::<f64>()) }
}

#[cfg(feature = "realm-http")]
unsafe extern "C" fn allocates_http_request_id(context: *mut c_void) -> i32 {
    // SAFETY: the test supplies one live `u64` slot while this callback runs.
    unsafe { kiln_realm_http_client_request_id_active(context.cast::<u64>()) }
}

#[cfg(feature = "realm-http")]
unsafe extern "C" fn exercises_http_agent(context: *mut c_void) -> i32 {
    if context.is_null() {
        return INVALID_ARGUMENT;
    }
    let host = b"Example.COM";
    let mut resource = 0;
    let acquired = unsafe {
        kiln_realm_http_agent_acquire_active(
            host.as_ptr(),
            host.len() as u64,
            80,
            1,
            &raw mut resource,
        )
    };
    if acquired != HTTP_AGENT_CONNECT || resource != 0 {
        return RUNTIME_ERROR;
    }
    if unsafe {
        kiln_realm_http_agent_connected_active(host.as_ptr(), host.len() as u64, 80, 91, 1)
    } != 1
    {
        return RUNTIME_ERROR;
    }
    let mut request = 0;
    if unsafe { kiln_realm_http_agent_release_active(91, 1, &raw mut request) } != HTTP_AGENT_IDLE {
        return RUNTIME_ERROR;
    }
    let reused = unsafe {
        kiln_realm_http_agent_acquire_active(
            host.as_ptr(),
            host.len() as u64,
            80,
            2,
            &raw mut resource,
        )
    };
    if reused != HTTP_AGENT_REUSE || resource != 91 {
        return RUNTIME_ERROR;
    }
    unsafe { context.cast::<u64>().write(resource) };
    0
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
#[cfg(feature = "realm-http")]
fn http_client_request_ids_are_monotonic_and_realm_scoped() {
    let mut first_realm = 0;
    let mut second_realm = 0;
    assert_eq!(unsafe { kiln_realm_create(&raw mut first_realm) }, 0);
    assert_eq!(unsafe { kiln_realm_create(&raw mut second_realm) }, 0);

    let mut first = 0;
    let mut second = 0;
    let mut other_realm = 0;
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                first_realm,
                Some(allocates_http_request_id),
                (&raw mut first).cast(),
            )
        },
        0
    );
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                first_realm,
                Some(allocates_http_request_id),
                (&raw mut second).cast(),
            )
        },
        0
    );
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                second_realm,
                Some(allocates_http_request_id),
                (&raw mut other_realm).cast(),
            )
        },
        0
    );
    assert_eq!((first, second, other_realm), (1, 2, 1));
    assert_eq!(kiln_realm_destroy(first_realm), 0);
    assert_eq!(kiln_realm_destroy(second_realm), 0);
}

#[test]
#[cfg(feature = "realm-http")]
fn native_http_agent_abi_maps_runtime_decisions_without_rust_layout() {
    let mut handle = 0;
    let mut reused = 0;
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    assert_eq!(
        unsafe { kiln_realm_enter(handle, Some(exercises_http_agent), (&raw mut reused).cast()) },
        0
    );
    assert_eq!(reused, 91);
    assert_eq!(kiln_realm_destroy(handle), 0);
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
    // SAFETY: no Realm is active, so the opaque callback cannot be accepted.
    assert_eq!(
        unsafe { kiln_realm_queue_next_tick(None, None, ptr::null_mut()) },
        INVALID_ARGUMENT
    );
}

#[test]
fn async_read_returns_identity_and_resumes_only_during_drive() {
    let suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("kiln-native-async-{suffix}.txt"));
    assert!(fs::write(&path, b"native continuation").is_ok());
    let path_bytes = path.to_string_lossy().as_bytes().to_vec();
    let mut handle = 0;
    let mut request = 0;
    let mut observation = ReadObservation {
        status: -1,
        request: 0,
        sequence: 0,
        bytes: Vec::new(),
        error_code: Vec::new(),
        drops: 0,
    };
    let mut submission = ReadSubmission {
        handle: 0,
        path: path_bytes,
        observation: &raw mut observation,
        request: &raw mut request,
    };
    // SAFETY: all slots and callback state remain live until after drive and teardown.
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    submission.handle = handle;
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                handle,
                Some(submit_read),
                (&raw mut submission).cast::<c_void>(),
            )
        },
        0
    );
    assert_ne!(request, 0);
    assert_eq!(observation.status, -1);
    let mut host = 0_u8;
    let mut thrown = 0_u8;
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
    assert_eq!(observation.status, 0);
    assert_eq!(observation.request, request);
    assert_eq!(observation.sequence, 0);
    assert_eq!(observation.bytes, b"native continuation");
    assert_eq!(observation.drops, 1);
    assert_eq!(kiln_realm_destroy(handle), 0);
    assert!(fs::remove_file(path).is_ok());
}

#[test]
fn native_timer_is_not_starved_by_pending_async_io() {
    let suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("kiln-native-turn-{suffix}.txt"));
    assert!(fs::write(&path, b"completion").is_ok());
    let mut handle = 0;
    let mut request = 0;
    let mut observation = ReadObservation {
        status: -1,
        request: 0,
        sequence: 0,
        bytes: Vec::new(),
        error_code: Vec::new(),
        drops: 0,
    };
    let mut order = Vec::new();
    let mut timer_drops = 0;
    let mut submission = ReadAndTimerSubmission {
        read: ReadSubmission {
            handle: 0,
            path: path.to_string_lossy().as_bytes().to_vec(),
            observation: &raw mut observation,
            request: &raw mut request,
        },
        order: &raw mut order,
        drops: &raw mut timer_drops,
    };
    // SAFETY: callback state remains live through drive and realm teardown.
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    submission.read.handle = handle;
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                handle,
                Some(submit_read_and_timer),
                (&raw mut submission).cast::<c_void>(),
            )
        },
        0
    );
    let mut host = 0_u8;
    let mut thrown = 0_u8;
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
    assert_eq!(order, [9]);
    assert_eq!(observation.status, 0);
    assert_eq!(observation.bytes, b"completion");
    assert_eq!(observation.drops, 1);
    assert_eq!(kiln_realm_destroy(handle), 0);
    assert_eq!(timer_drops, 1);
    assert!(fs::remove_file(path).is_ok());
}

#[test]
fn generic_native_host_ops_exchange_live_tcp_bytes_and_resources() {
    struct TcpExchange {
        handle: u64,
        stage: u8,
        listener: HostObservation,
        accepted: HostObservation,
        client: HostObservation,
        write: HostObservation,
        read: HostObservation,
        unref: HostObservation,
        state: HostObservation,
        close_listener: HostObservation,
        close_client: HostObservation,
        close_accepted: HostObservation,
        requests: [u64; 10],
    }

    unsafe extern "C" fn submit_stage(context: *mut c_void) -> i32 {
        if context.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the test keeps this exchange live through every entry and drive.
        let exchange = unsafe { &mut *context.cast::<TcpExchange>() };
        let host = b"127.0.0.1";
        match exchange.stage {
            0 => {
                let operation = HostOpView {
                    operation: OP_TCP_LISTEN,
                    flags: 1,
                    resource: 0,
                    argument: 0,
                    text: host.as_ptr(),
                    text_len: host.len() as u64,
                    bytes: ptr::null(),
                    bytes_len: 0,
                };
                // SAFETY: operation framing and callback targets are live.
                unsafe {
                    start_test_host_op(
                        exchange.handle,
                        &operation,
                        &raw mut exchange.listener,
                        &raw mut exchange.requests[0],
                    )
                }
            }
            1 => {
                let accept = HostOpView {
                    operation: OP_TCP_ACCEPT,
                    flags: 1,
                    resource: exchange.listener.resource,
                    argument: 0,
                    text: ptr::null(),
                    text_len: 0,
                    bytes: ptr::null(),
                    bytes_len: 0,
                };
                let connect = HostOpView {
                    operation: OP_TCP_CONNECT,
                    flags: 1,
                    resource: 0,
                    argument: u64::from(exchange.listener.resource_port),
                    text: host.as_ptr(),
                    text_len: host.len() as u64,
                    bytes: ptr::null(),
                    bytes_len: 0,
                };
                // SAFETY: both operations copy their frames before returning.
                let status = unsafe {
                    start_test_host_op(
                        exchange.handle,
                        &accept,
                        &raw mut exchange.accepted,
                        &raw mut exchange.requests[1],
                    )
                };
                if status != 0 {
                    return status;
                }
                // SAFETY: callback targets remain live through the following drive.
                unsafe {
                    start_test_host_op(
                        exchange.handle,
                        &connect,
                        &raw mut exchange.client,
                        &raw mut exchange.requests[2],
                    )
                }
            }
            2 => {
                let bytes = b"native tcp";
                let write = HostOpView {
                    operation: OP_TCP_WRITE,
                    flags: 1,
                    resource: exchange.client.resource,
                    argument: 0,
                    text: ptr::null(),
                    text_len: 0,
                    bytes: bytes.as_ptr(),
                    bytes_len: bytes.len() as u64,
                };
                let read = HostOpView {
                    operation: OP_TCP_READ,
                    flags: 1,
                    resource: exchange.accepted.resource,
                    argument: 64,
                    text: ptr::null(),
                    text_len: 0,
                    bytes: ptr::null(),
                    bytes_len: 0,
                };
                // SAFETY: the write bytes are copied and callback targets remain live.
                let status = unsafe {
                    start_test_host_op(
                        exchange.handle,
                        &read,
                        &raw mut exchange.read,
                        &raw mut exchange.requests[3],
                    )
                };
                if status != 0 {
                    return status;
                }
                // SAFETY: operation framing is live through submission.
                unsafe {
                    start_test_host_op(
                        exchange.handle,
                        &write,
                        &raw mut exchange.write,
                        &raw mut exchange.requests[4],
                    )
                }
            }
            3 => {
                let unref = HostOpView {
                    operation: OP_SET_RESOURCE_REFERENCED,
                    flags: 0,
                    resource: exchange.client.resource,
                    argument: 0,
                    text: ptr::null(),
                    text_len: 0,
                    bytes: ptr::null(),
                    bytes_len: 0,
                };
                // SAFETY: operation framing is live through submission.
                unsafe {
                    start_test_host_op(
                        exchange.handle,
                        &unref,
                        &raw mut exchange.unref,
                        &raw mut exchange.requests[5],
                    )
                }
            }
            4 => {
                let state = HostOpView {
                    operation: OP_RESOURCE_STATE,
                    flags: 0,
                    resource: exchange.client.resource,
                    argument: 0,
                    text: ptr::null(),
                    text_len: 0,
                    bytes: ptr::null(),
                    bytes_len: 0,
                };
                // SAFETY: operation framing is live through submission.
                unsafe {
                    start_test_host_op(
                        exchange.handle,
                        &state,
                        &raw mut exchange.state,
                        &raw mut exchange.requests[6],
                    )
                }
            }
            5 => {
                let resources = [
                    (exchange.listener.resource, &raw mut exchange.close_listener),
                    (exchange.client.resource, &raw mut exchange.close_client),
                    (exchange.accepted.resource, &raw mut exchange.close_accepted),
                ];
                for (offset, (resource, observation)) in resources.into_iter().enumerate() {
                    let close = HostOpView {
                        operation: OP_CLOSE_RESOURCE,
                        flags: 0,
                        resource,
                        argument: 0,
                        text: ptr::null(),
                        text_len: 0,
                        bytes: ptr::null(),
                        bytes_len: 0,
                    };
                    // SAFETY: each operation is copied and each callback target remains
                    // live through the following drive.
                    let handle = exchange.handle;
                    let Some(request) = exchange.requests.get_mut(7 + offset) else {
                        return INVALID_ARGUMENT;
                    };
                    let status = unsafe {
                        start_test_host_op(handle, &close, observation, &raw mut *request)
                    };
                    if status != 0 {
                        return status;
                    }
                }
                0
            }
            _ => INVALID_ARGUMENT,
        }
    }

    let mut handle = 0;
    let mut exchange = TcpExchange {
        handle: 0,
        stage: 0,
        listener: HostObservation::default(),
        accepted: HostObservation::default(),
        client: HostObservation::default(),
        write: HostObservation::default(),
        read: HostObservation::default(),
        unref: HostObservation::default(),
        state: HostObservation::default(),
        close_listener: HostObservation::default(),
        close_client: HostObservation::default(),
        close_accepted: HostObservation::default(),
        requests: [0; 10],
    };
    // SAFETY: handle and exchange storage remain live through the complete test.
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    exchange.handle = handle;
    let mut host = 0_u8;
    let mut thrown = 0_u8;
    for stage in 0..=5 {
        exchange.stage = stage;
        // SAFETY: callback and context satisfy their contracts.
        assert_eq!(
            unsafe {
                kiln_realm_enter(
                    handle,
                    Some(submit_stage),
                    (&raw mut exchange).cast::<c_void>(),
                )
            },
            0
        );
        // SAFETY: host and thrown slots remain live for every continuation in this drive.
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
    }

    assert_eq!(exchange.listener.value_kind, VALUE_RESOURCE);
    assert_eq!(exchange.listener.resource_kind, RESOURCE_LISTENER);
    assert_eq!(exchange.listener.address, b"127.0.0.1");
    assert_ne!(exchange.listener.resource_port, 0);
    assert_eq!(exchange.accepted.value_kind, VALUE_ACCEPTED_STREAM);
    assert_eq!(exchange.accepted.resource_kind, RESOURCE_STREAM);
    assert_eq!(exchange.accepted.peer_address, b"127.0.0.1");
    assert_eq!(exchange.client.value_kind, VALUE_RESOURCE);
    assert_eq!(exchange.client.resource_kind, RESOURCE_STREAM);
    assert_eq!(exchange.write.value_kind, VALUE_UNDEFINED);
    assert_eq!(exchange.read.value_kind, VALUE_BYTES);
    assert_eq!(exchange.read.bytes, b"native tcp");
    assert_eq!(exchange.unref.value_kind, VALUE_UNDEFINED);
    assert_eq!(exchange.state.value_kind, VALUE_RESOURCE_STATE);
    assert_eq!(exchange.state.resource, exchange.client.resource);
    assert_eq!(exchange.state.resource_kind, RESOURCE_STREAM);
    assert_eq!(exchange.state.resource_flags & FLAG_REFERENCED, 0);
    assert_eq!(exchange.close_listener.value_kind, VALUE_UNDEFINED);
    assert_eq!(exchange.close_client.value_kind, VALUE_UNDEFINED);
    assert_eq!(exchange.close_accepted.value_kind, VALUE_UNDEFINED);
    assert!(exchange.requests.into_iter().all(|request| request != 0));
    assert_eq!(kiln_realm_destroy(handle), 0);
    assert_eq!(exchange.listener.drops, 1);
    assert_eq!(exchange.accepted.drops, 1);
    assert_eq!(exchange.client.drops, 1);
    assert_eq!(exchange.write.drops, 1);
    assert_eq!(exchange.read.drops, 1);
    assert_eq!(exchange.unref.drops, 1);
    assert_eq!(exchange.state.drops, 1);
    assert_eq!(exchange.close_listener.drops, 1);
    assert_eq!(exchange.close_client.drops, 1);
    assert_eq!(exchange.close_accepted.drops, 1);
}

#[test]
fn cancelling_queued_accept_retires_its_resource_but_state_query_does_not() {
    use std::io::Read;
    use std::net::TcpStream;
    use std::time::Duration;

    struct Exchange {
        handle: u64,
        stage: u8,
        listener: HostObservation,
        trigger: HostObservation,
        borrowed_query: HostObservation,
        accepted: HostObservation,
        accepted_after_cancel: HostObservation,
        listener_after_cancel: HostObservation,
        close_listener: HostObservation,
        peer: Option<TcpStream>,
        requests: [u64; 7],
        trigger_callback_ran: usize,
        cancel_query: i32,
        cancel_accept: i32,
        accepted_resource: u64,
        cancel_work_drops: usize,
    }

    struct CancelQueuedWork {
        observation: *mut HostObservation,
        handle: u64,
        borrowed_request: *const u64,
        accepted_request: *const u64,
        cancel_query: *mut i32,
        cancel_accept: *mut i32,
        accepted_resource: *mut u64,
        callback_ran: *mut usize,
        drop_count: *mut usize,
    }

    unsafe extern "C" fn cancel_queued_next_tick(
        pointer: *mut c_void,
        _host: *mut c_void,
        _thrown: *mut c_void,
    ) -> i32 {
        if pointer.is_null() {
            return INVALID_ARGUMENT;
        }
        let work = unsafe { &mut *pointer.cast::<CancelQueuedWork>() };
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let accept_request = unsafe { *work.accepted_request };
        let resource = realm
            .queued
            .borrow()
            .get(&RequestId::new(accept_request))
            .copied()
            .flatten();
        unsafe { *work.accepted_resource = resource.map_or(0, ResourceId::get) };
        unsafe {
            *work.cancel_query = kiln_realm_cancel_request(work.handle, *work.borrowed_request);
            *work.cancel_accept = kiln_realm_cancel_request(work.handle, accept_request);
        }
        0
    }

    unsafe extern "C" fn cancel_queued_completions(
        pointer: *mut c_void,
        completion: *const CompletionView,
        host: *mut c_void,
        thrown: *mut c_void,
    ) -> i32 {
        if pointer.is_null() {
            return INVALID_ARGUMENT;
        }
        let work = unsafe { &mut *pointer.cast::<CancelQueuedWork>() };
        let mut host_work = HostWork {
            observation: work.observation,
        };
        let status = unsafe {
            record_host_completion((&raw mut host_work).cast(), completion, host, thrown)
        };
        if status != 0 {
            return status;
        }
        unsafe { *work.callback_ran += 1 };
        let followup = CancelQueuedWork {
            observation: ptr::null_mut(),
            handle: work.handle,
            borrowed_request: work.borrowed_request,
            accepted_request: work.accepted_request,
            cancel_query: work.cancel_query,
            cancel_accept: work.cancel_accept,
            accepted_resource: work.accepted_resource,
            callback_ran: work.callback_ran,
            drop_count: work.drop_count,
        };
        let next_tick = Box::into_raw(Box::new(followup)).cast::<c_void>();
        let status = unsafe {
            kiln_realm_queue_next_tick(
                Some(cancel_queued_next_tick),
                Some(drop_cancel_queued_work),
                next_tick,
            )
        };
        if status != 0 {
            unsafe { drop_cancel_queued_work(next_tick) };
        }
        status
    }

    unsafe extern "C" fn drop_cancel_queued_work(pointer: *mut c_void) {
        if pointer.is_null() {
            return;
        }
        let work = unsafe { Box::from_raw(pointer.cast::<CancelQueuedWork>()) };
        if !work.observation.is_null() {
            unsafe {
                (*work.observation).drops = (*work.observation).drops.saturating_add(1);
            }
        }
        if !work.drop_count.is_null() {
            unsafe {
                *work.drop_count = (*work.drop_count).saturating_add(1);
            }
        }
    }

    unsafe fn start_cancel_queued_op(
        handle: u64,
        operation: &HostOpView,
        work: CancelQueuedWork,
        request: *mut u64,
    ) -> i32 {
        let pointer = Box::into_raw(Box::new(work)).cast::<c_void>();
        let status = unsafe {
            kiln_realm_start_host_op(
                handle,
                operation,
                Some(cancel_queued_completions),
                Some(drop_cancel_queued_work),
                pointer,
                request,
            )
        };
        if status != 0 {
            unsafe { drop_cancel_queued_work(pointer) };
        }
        status
    }

    unsafe extern "C" fn submit_stage(context: *mut c_void) -> i32 {
        if context.is_null() {
            return INVALID_ARGUMENT;
        }
        let exchange = unsafe { &mut *context.cast::<Exchange>() };
        let host = b"127.0.0.1";
        match exchange.stage {
            0 => {
                let listen = HostOpView {
                    operation: OP_TCP_LISTEN,
                    flags: 1,
                    resource: 0,
                    argument: 0,
                    text: host.as_ptr(),
                    text_len: host.len() as u64,
                    bytes: ptr::null(),
                    bytes_len: 0,
                };
                unsafe {
                    start_test_host_op(
                        exchange.handle,
                        &listen,
                        &raw mut exchange.listener,
                        &raw mut exchange.requests[0],
                    )
                }
            }
            1 => {
                let mut peer =
                    match TcpStream::connect(("127.0.0.1", exchange.listener.resource_port)) {
                        Ok(peer) => peer,
                        Err(_) => return RUNTIME_ERROR,
                    };
                let _ = peer.set_read_timeout(Some(Duration::from_secs(2)));
                exchange.peer = Some(peer);
                let mut state = HostOpView {
                    operation: OP_RESOURCE_STATE,
                    flags: 0,
                    resource: exchange.listener.resource,
                    argument: 0,
                    text: ptr::null(),
                    text_len: 0,
                    bytes: ptr::null(),
                    bytes_len: 0,
                };
                let work = CancelQueuedWork {
                    observation: &raw mut exchange.trigger,
                    handle: exchange.handle,
                    borrowed_request: &raw const exchange.requests[2],
                    accepted_request: &raw const exchange.requests[3],
                    cancel_query: &raw mut exchange.cancel_query,
                    cancel_accept: &raw mut exchange.cancel_accept,
                    accepted_resource: &raw mut exchange.accepted_resource,
                    callback_ran: &raw mut exchange.trigger_callback_ran,
                    drop_count: &raw mut exchange.cancel_work_drops,
                };
                let status = unsafe {
                    start_cancel_queued_op(
                        exchange.handle,
                        &state,
                        work,
                        &raw mut exchange.requests[1],
                    )
                };
                if status != 0 {
                    return status;
                }
                let status = unsafe {
                    start_test_host_op(
                        exchange.handle,
                        &state,
                        &raw mut exchange.borrowed_query,
                        &raw mut exchange.requests[2],
                    )
                };
                if status != 0 {
                    return status;
                }
                let accept = HostOpView {
                    operation: OP_TCP_ACCEPT,
                    flags: 1,
                    resource: exchange.listener.resource,
                    argument: 0,
                    text: ptr::null(),
                    text_len: 0,
                    bytes: ptr::null(),
                    bytes_len: 0,
                };
                let status = unsafe {
                    start_test_host_op(
                        exchange.handle,
                        &accept,
                        &raw mut exchange.accepted,
                        &raw mut exchange.requests[3],
                    )
                };
                if status != 0 {
                    return status;
                }
                struct NoopWake;
                impl std::task::Wake for NoopWake {
                    fn wake(self: std::sync::Arc<Self>) {}
                    fn wake_by_ref(self: &std::sync::Arc<Self>) {}
                }
                let waker = std::task::Waker::from(std::sync::Arc::new(NoopWake));
                if let Some(realm) = active_realm() {
                    realm.runtime.executor().poll_local_once(&waker);
                    realm.runtime.executor().poll_local_once(&waker);
                } else {
                    return INVALID_ARGUMENT;
                }
                0
            }
            2 => {
                let state_accepted = HostOpView {
                    operation: OP_RESOURCE_STATE,
                    flags: 0,
                    resource: exchange.accepted_resource,
                    argument: 0,
                    text: ptr::null(),
                    text_len: 0,
                    bytes: ptr::null(),
                    bytes_len: 0,
                };
                let status = unsafe {
                    start_test_host_op(
                        exchange.handle,
                        &state_accepted,
                        &raw mut exchange.accepted_after_cancel,
                        &raw mut exchange.requests[4],
                    )
                };
                if status != 0 {
                    return status;
                }
                let state_listener = HostOpView {
                    operation: OP_RESOURCE_STATE,
                    flags: 0,
                    resource: exchange.listener.resource,
                    argument: 0,
                    text: ptr::null(),
                    text_len: 0,
                    bytes: ptr::null(),
                    bytes_len: 0,
                };
                unsafe {
                    start_test_host_op(
                        exchange.handle,
                        &state_listener,
                        &raw mut exchange.listener_after_cancel,
                        &raw mut exchange.requests[5],
                    )
                }
            }
            3 => {
                let close = HostOpView {
                    operation: OP_CLOSE_RESOURCE,
                    flags: 0,
                    resource: exchange.listener.resource,
                    argument: 0,
                    text: ptr::null(),
                    text_len: 0,
                    bytes: ptr::null(),
                    bytes_len: 0,
                };
                unsafe {
                    start_test_host_op(
                        exchange.handle,
                        &close,
                        &raw mut exchange.close_listener,
                        &raw mut exchange.requests[6],
                    )
                }
            }
            _ => INVALID_ARGUMENT,
        }
    }

    let mut handle = 0;
    let mut exchange = Exchange {
        handle: 0,
        stage: 0,
        listener: HostObservation::default(),
        trigger: HostObservation::default(),
        borrowed_query: HostObservation::default(),
        accepted: HostObservation::default(),
        accepted_after_cancel: HostObservation::default(),
        listener_after_cancel: HostObservation::default(),
        close_listener: HostObservation::default(),
        peer: None,
        requests: [0; 7],
        trigger_callback_ran: 0,
        cancel_query: -1,
        cancel_accept: -1,
        accepted_resource: 0,
        cancel_work_drops: 0,
    };
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    exchange.handle = handle;
    let mut host = 0_u8;
    let mut thrown = 0_u8;
    for stage in 0..=3 {
        exchange.stage = stage;
        assert_eq!(
            unsafe {
                kiln_realm_enter(
                    handle,
                    Some(submit_stage),
                    (&raw mut exchange).cast::<c_void>(),
                )
            },
            0
        );
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
        if stage == 1 {
            assert_eq!(exchange.trigger_callback_ran, 1);
            assert_eq!(exchange.cancel_query, 0);
            assert_eq!(exchange.cancel_accept, 0);
            assert_ne!(exchange.accepted_resource, 0);
            assert_eq!(exchange.trigger.value_kind, VALUE_RESOURCE_STATE);
            assert_eq!(exchange.borrowed_query.value_kind, 0);
            assert_eq!(exchange.accepted.value_kind, 0);
            assert_eq!(exchange.trigger.drops, 1);
            assert_eq!(exchange.borrowed_query.drops, 1);
            assert_eq!(exchange.accepted.drops, 1);
            assert_eq!(exchange.cancel_work_drops, 2);
            let mut byte = [0_u8; 1];
            assert_eq!(exchange.peer.as_mut().unwrap().read(&mut byte).unwrap(), 0);
        }
    }
    assert_ne!(exchange.accepted_after_cancel.status, 0);
    assert_eq!(exchange.listener_after_cancel.status, 0);
    assert_eq!(
        exchange.listener_after_cancel.value_kind,
        VALUE_RESOURCE_STATE
    );
    assert_eq!(exchange.close_listener.status, 0);
    assert_eq!(kiln_realm_destroy(handle), 0);
}

#[test]
fn active_host_op_submission_needs_no_provider_handle() {
    struct Submission {
        observation: *mut HostObservation,
        request: *mut u64,
    }

    unsafe extern "C" fn submit(context: *mut c_void) -> i32 {
        if context.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the test owns this context for the complete realm entry.
        let submission = unsafe { &mut *context.cast::<Submission>() };
        let path = b"definitely-missing-kiln-active-op";
        let operation = HostOpView {
            operation: OP_STAT,
            flags: 1,
            resource: 0,
            argument: 0,
            text: path.as_ptr(),
            text_len: path.len() as u64,
            bytes: ptr::null(),
            bytes_len: 0,
        };
        // SAFETY: the frame is copied synchronously and both targets outlive drive.
        unsafe { start_test_active_host_op(&operation, submission.observation, submission.request) }
    }

    let mut handle = 0;
    let mut observation = HostObservation::default();
    let mut request = 0;
    let mut submission = Submission {
        observation: &raw mut observation,
        request: &raw mut request,
    };
    // SAFETY: all callback storage remains live through realm drive.
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    assert_eq!(
        unsafe { kiln_realm_enter(handle, Some(submit), (&raw mut submission).cast::<c_void>()) },
        0
    );
    assert_ne!(request, 0);
    assert_eq!(
        unsafe { kiln_realm_drive(handle, ptr::null_mut(), ptr::null_mut()) },
        0
    );
    assert_ne!(observation.status, 0);
    assert_eq!(observation.drops, 1);
    assert_eq!(kiln_realm_destroy(handle), 0);
}

#[test]
fn async_read_delivers_structured_error_and_drops_continuation() {
    let suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("kiln-native-missing-{suffix}.txt"));
    let mut handle = 0;
    let mut request = 0;
    let mut observation = ReadObservation {
        status: -1,
        request: 0,
        sequence: 0,
        bytes: Vec::new(),
        error_code: Vec::new(),
        drops: 0,
    };
    let mut submission = ReadSubmission {
        handle: 0,
        path: path.to_string_lossy().as_bytes().to_vec(),
        observation: &raw mut observation,
        request: &raw mut request,
    };
    // SAFETY: all callback state remains live through the complete request drive.
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    submission.handle = handle;
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                handle,
                Some(submit_read),
                (&raw mut submission).cast::<c_void>(),
            )
        },
        0
    );
    let mut host = 0_u8;
    let mut thrown = 0_u8;
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
    assert_eq!(observation.status, 1);
    assert_eq!(observation.request, request);
    assert_eq!(observation.error_code, b"ENOENT");
    assert_eq!(observation.drops, 1);
    assert_eq!(kiln_realm_destroy(handle), 0);
}

#[test]
fn async_read_cancellation_prevents_resume_and_releases_continuation() {
    let suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("kiln-native-cancel-{suffix}.txt"));
    assert!(fs::write(&path, b"canceled").is_ok());
    let mut handle = 0;
    let mut request = 0;
    let mut observation = ReadObservation {
        status: -1,
        request: 0,
        sequence: 0,
        bytes: Vec::new(),
        error_code: Vec::new(),
        drops: 0,
    };
    let mut submission = ReadSubmission {
        handle: 0,
        path: path.to_string_lossy().as_bytes().to_vec(),
        observation: &raw mut observation,
        request: &raw mut request,
    };
    // SAFETY: all callback state remains live through cancellation, drive, and teardown.
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    submission.handle = handle;
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                handle,
                Some(submit_and_cancel),
                (&raw mut submission).cast::<c_void>(),
            )
        },
        0
    );
    assert_ne!(request, 0);
    assert_eq!(observation.drops, 1);
    let mut host = 0_u8;
    let mut thrown = 0_u8;
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
    assert_eq!(observation.status, -1);
    assert_eq!(kiln_realm_destroy(handle), 0);
    assert!(fs::remove_file(path).is_ok());
}

#[test]
fn realm_destruction_releases_pending_async_continuations() {
    let suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("kiln-native-drop-{suffix}.txt"));
    assert!(fs::write(&path, b"dropped").is_ok());
    let mut handle = 0;
    let mut request = 0;
    let mut observation = ReadObservation {
        status: -1,
        request: 0,
        sequence: 0,
        bytes: Vec::new(),
        error_code: Vec::new(),
        drops: 0,
    };
    let mut submission = ReadSubmission {
        handle: 0,
        path: path.to_string_lossy().as_bytes().to_vec(),
        observation: &raw mut observation,
        request: &raw mut request,
    };
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    submission.handle = handle;
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                handle,
                Some(submit_read),
                (&raw mut submission).cast::<c_void>(),
            )
        },
        0
    );
    assert_ne!(request, 0);
    assert_eq!(observation.drops, 0);
    assert_eq!(kiln_realm_destroy(handle), 0);
    assert_eq!(observation.drops, 1);
    assert_eq!(observation.status, -1);
    assert!(fs::remove_file(path).is_ok());
}

#[test]
fn async_reads_use_submission_order_when_ready_together() {
    let first_suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let second_suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let first = std::env::temp_dir().join(format!("kiln-native-order-{first_suffix}.txt"));
    let second = std::env::temp_dir().join(format!("kiln-native-order-{second_suffix}.txt"));
    assert!(fs::write(&first, b"first").is_ok());
    assert!(fs::write(&second, b"second").is_ok());
    let mut handle = 0;
    let mut first_request = 0;
    let mut second_request = 0;
    let mut first_observation = ReadObservation {
        status: -1,
        request: 0,
        sequence: 0,
        bytes: Vec::new(),
        error_code: Vec::new(),
        drops: 0,
    };
    let mut second_observation = ReadObservation {
        status: -1,
        request: 0,
        sequence: 0,
        bytes: Vec::new(),
        error_code: Vec::new(),
        drops: 0,
    };
    let mut first_submission = ReadSubmission {
        handle: 0,
        path: first.to_string_lossy().as_bytes().to_vec(),
        observation: &raw mut first_observation,
        request: &raw mut first_request,
    };
    let mut second_submission = ReadSubmission {
        handle: 0,
        path: second.to_string_lossy().as_bytes().to_vec(),
        observation: &raw mut second_observation,
        request: &raw mut second_request,
    };
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    first_submission.handle = handle;
    second_submission.handle = handle;
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                handle,
                Some(submit_read),
                (&raw mut first_submission).cast::<c_void>(),
            )
        },
        0
    );
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                handle,
                Some(submit_read),
                (&raw mut second_submission).cast::<c_void>(),
            )
        },
        0
    );
    let mut host = 0_u8;
    let mut thrown = 0_u8;
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
    assert_eq!(first_observation.sequence, 0);
    assert_eq!(second_observation.sequence, 1);
    assert_eq!(first_observation.bytes, b"first");
    assert_eq!(second_observation.bytes, b"second");
    assert_eq!(kiln_realm_destroy(handle), 0);
    assert!(fs::remove_file(first).is_ok());
    assert!(fs::remove_file(second).is_ok());
}

#[test]
fn async_request_rejects_cross_realm_and_stale_cancellation() {
    let suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("kiln-native-realm-{suffix}.txt"));
    assert!(fs::write(&path, b"realm").is_ok());
    let mut first_handle = 0;
    let mut second_handle = 0;
    assert_eq!(unsafe { kiln_realm_create(&raw mut first_handle) }, 0);
    assert_eq!(unsafe { kiln_realm_create(&raw mut second_handle) }, 0);
    let mut request = 0;
    let mut observation = ReadObservation {
        status: -1,
        request: 0,
        sequence: 0,
        bytes: Vec::new(),
        error_code: Vec::new(),
        drops: 0,
    };
    let mut submission = ReadSubmission {
        handle: first_handle,
        path: path.to_string_lossy().as_bytes().to_vec(),
        observation: &raw mut observation,
        request: &raw mut request,
    };
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                first_handle,
                Some(submit_read),
                (&raw mut submission).cast::<c_void>(),
            )
        },
        0
    );
    let mut cross_realm_result = -1;
    let mut cross_realm = CancelSubmission {
        handle: first_handle,
        request,
        result: &raw mut cross_realm_result,
    };
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                second_handle,
                Some(cancel_request),
                (&raw mut cross_realm).cast::<c_void>(),
            )
        },
        0
    );
    assert_eq!(cross_realm_result, INVALID_ARGUMENT);
    let mut host = 0_u8;
    let mut thrown = 0_u8;
    assert_eq!(
        unsafe {
            kiln_realm_drive(
                first_handle,
                (&raw mut host).cast::<c_void>(),
                (&raw mut thrown).cast::<c_void>(),
            )
        },
        0
    );
    assert_eq!(observation.status, 0);
    let mut stale_result = -1;
    let mut stale = CancelSubmission {
        handle: first_handle,
        request,
        result: &raw mut stale_result,
    };
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                first_handle,
                Some(cancel_request),
                (&raw mut stale).cast::<c_void>(),
            )
        },
        0
    );
    assert_eq!(stale_result, 1);
    assert_eq!(kiln_realm_destroy(first_handle), 0);
    assert_eq!(kiln_realm_destroy(second_handle), 0);
    assert!(fs::remove_file(path).is_ok());
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

unsafe extern "C" fn arms_short_timer(context: *mut c_void) -> i32 {
    // SAFETY: the test supplies one live `Instant` slot for the timer to stamp.
    let fired = context.cast::<Option<std::time::Instant>>();
    unsafe extern "C" fn stamp(
        context: *mut c_void,
        _host: *mut c_void,
        _thrown: *mut c_void,
    ) -> i32 {
        // SAFETY: the slot outlives the drive that runs this timer.
        unsafe {
            context
                .cast::<Option<std::time::Instant>>()
                .write(Some(std::time::Instant::now()))
        };
        0
    }
    unsafe extern "C" fn no_drop(_: *mut c_void) {}
    let mut id = 0;
    // SAFETY: the callbacks and the slot stay live through the drive.
    unsafe { kiln_realm_set_timeout(30.0, Some(stamp), Some(no_drop), fired.cast(), &raw mut id) }
}

#[test]
fn wall_clock_realm_reads_the_host_and_fires_timers_on_it() -> Result<(), Box<dyn Error>> {
    let mut handle = 0;
    let mut now = f64::NAN;
    let mut performance_now = f64::NAN;
    let mut fired: Option<std::time::Instant> = None;
    // SAFETY: every slot outlives the calls that use it.
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    assert_eq!(kiln_realm_use_wall_clock(0), INVALID_ARGUMENT);
    assert_eq!(kiln_realm_use_wall_clock(handle), 0);
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as f64;
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
    assert!(
        now >= before,
        "wall clock {now} reads before the test started {before}"
    );
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                handle,
                Some(reads_performance_clock),
                (&raw mut performance_now).cast::<c_void>(),
            )
        },
        0
    );
    assert!(
        (0.0..10_000.0).contains(&performance_now),
        "performance clock must be elapsed time, got {performance_now}"
    );
    let armed = std::time::Instant::now();
    assert_eq!(
        unsafe {
            kiln_realm_enter(
                handle,
                Some(arms_short_timer),
                (&raw mut fired).cast::<c_void>(),
            )
        },
        0
    );
    let mut host = 0_u8;
    let mut thrown = 0_u8;
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
    let fired = fired.ok_or("the timer ran during the drive")?;
    // Virtual time would have jumped the 30 ms; wall time waits for them.
    assert!(fired.duration_since(armed) >= std::time::Duration::from_millis(30));
    assert_eq!(kiln_realm_destroy(handle), 0);
    Ok(())
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
    assert_eq!(observed.0, [5, 6, 1, 2, 3, 4]);
    assert_eq!(kiln_realm_destroy(handle), 0);
    assert_eq!(observed.1, 6);
}

#[test]
fn thrown_next_tick_drops_its_payload_before_drive_returns() {
    let mut handle = 0;
    let mut observed = (Vec::<u8>::new(), 0_usize);
    // SAFETY: handle and context are live for the calls that use them.
    assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
    unsafe extern "C" fn schedule(context: *mut c_void) -> i32 {
        if context.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the test keeps this context live through Realm entry and drive.
        let (order, drops) = unsafe { &mut *context.cast::<(Vec<u8>, usize)>() };
        let work = recorded_work(7, order, drops);
        // SAFETY: callbacks match the transferred allocation.
        let status = unsafe {
            kiln_realm_queue_next_tick(Some(fail_recorded_work), Some(drop_recorded_work), work)
        };
        if status != 0 {
            // SAFETY: ownership did not transfer on failure.
            unsafe { drop_recorded_work(work) };
        }
        status
    }
    assert_eq!(
        unsafe { kiln_realm_enter(handle, Some(schedule), (&raw mut observed).cast::<c_void>()) },
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
        RUNTIME_ERROR
    );
    assert_eq!(observed.0, [7]);
    assert_eq!(kiln_realm_destroy(handle), 0);
    assert_eq!(observed.1, 1);
}

/// What one thread's realm saw while another thread's realm was active at the same moment.
struct ConcurrentRealmProbe {
    barrier: std::sync::Arc<std::sync::Barrier>,
    mark: u64,
    read_back: u64,
    generation_slot: usize,
}

unsafe extern "C" fn holds_its_slot_across_another_realm(context: *mut c_void) -> i32 {
    // SAFETY: the test passes a live probe for the duration of the entry.
    let probe = unsafe { &mut *context.cast::<ConcurrentRealmProbe>() };
    let mut slot = ptr::null_mut();
    if unsafe { kiln_realm_value_slot(1, &raw mut slot) } != 0 {
        return RUNTIME_ERROR;
    }
    unsafe { slot.cast::<u64>().write(probe.mark) };
    // Both realms are entered, on their own threads, when either passes this point.
    probe.barrier.wait();
    probe.read_back = unsafe { slot.cast::<u64>().read() };
    probe.generation_slot = kiln_realm_generation_slot() as usize;
    probe.barrier.wait();
    0
}

/// Realms issue 04 step 5: the active-realm registry is per thread, so two threads each run their
/// own realm at the same time, each sees only its own realm's state, and each has its own
/// generation word for the prelude's caches.
#[test]
fn realms_on_two_threads_are_active_at_once_and_each_sees_only_its_own() {
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let run = |mark: u64| {
        let barrier = std::sync::Arc::clone(&barrier);
        std::thread::spawn(move || {
            let mut handle = 0;
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            let mut probe = ConcurrentRealmProbe {
                barrier,
                mark,
                read_back: 0,
                generation_slot: 0,
            };
            let status = unsafe {
                kiln_realm_enter(
                    handle,
                    Some(holds_its_slot_across_another_realm),
                    (&raw mut probe).cast(),
                )
            };
            assert_eq!(kiln_realm_destroy(handle), 0);
            (status, probe.read_back, probe.generation_slot)
        })
    };
    let first = run(11);
    let second = run(22);
    let first = first.join().expect("first realm thread");
    let second = second.join().expect("second realm thread");
    assert_eq!((first.0, first.1), (0, 11));
    assert_eq!((second.0, second.1), (0, 22));
    assert_ne!(
        first.2, second.2,
        "each thread has its own realm generation"
    );
}

#[cfg(all(feature = "ffi", any(target_os = "linux", target_os = "macos")))]
mod realm_ffi {
    use super::*;

    #[cfg(target_os = "linux")]
    const LIBRARY: &[u8] = b"libc.so.6";
    #[cfg(target_os = "macos")]
    const LIBRARY: &[u8] = b"/usr/lib/libSystem.B.dylib";

    /// Opens the library and records the handle, or closes the handle it is given.
    struct FfiProbe {
        close: u64,
        close_status: i32,
        opened: u64,
    }

    unsafe extern "C" fn ffi_probe(context: *mut c_void) -> i32 {
        // SAFETY: the test passes a live probe for the duration of the entry.
        let probe = unsafe { &mut *context.cast::<FfiProbe>() };
        if probe.close != 0 {
            probe.close_status = crate::ffi::kiln_ffi_close(probe.close);
        }
        if probe.opened == 0 {
            let status = unsafe {
                crate::ffi::kiln_ffi_open(LIBRARY.as_ptr(), LIBRARY.len() as u64, &mut probe.opened)
            };
            if status != 0 {
                return RUNTIME_ERROR;
            }
        }
        0
    }

    /// Realms issue 04 step 5: a realm's `node:ffi` libraries are its own. Another realm cannot
    /// close one, and the handle space starts again in each realm.
    #[test]
    fn ffi_libraries_belong_to_the_realm_that_opened_them() {
        let (mut first, mut second) = (0, 0);
        assert_eq!(unsafe { kiln_realm_create(&raw mut first) }, 0);
        assert_eq!(unsafe { kiln_realm_create(&raw mut second) }, 0);
        let mut in_first = FfiProbe {
            close: 0,
            close_status: 0,
            opened: 0,
        };
        assert_eq!(
            unsafe { kiln_realm_enter(first, Some(ffi_probe), (&raw mut in_first).cast()) },
            0
        );
        assert_ne!(in_first.opened, 0);
        let mut in_second = FfiProbe {
            close: in_first.opened,
            close_status: 0,
            opened: 0,
        };
        assert_eq!(
            unsafe { kiln_realm_enter(second, Some(ffi_probe), (&raw mut in_second).cast()) },
            0
        );
        assert_ne!(
            in_second.close_status, 0,
            "the second realm cannot close the first's library"
        );
        assert_eq!(
            in_second.opened, in_first.opened,
            "handles are numbered per realm"
        );
        // Tearing the first realm down releases its library and leaves the second's open.
        assert_eq!(kiln_realm_destroy(first), 0);
        let mut again = FfiProbe {
            close: in_second.opened,
            close_status: -1,
            opened: 1,
        };
        assert_eq!(
            unsafe { kiln_realm_enter(second, Some(ffi_probe), (&raw mut again).cast()) },
            0
        );
        assert_eq!(again.close_status, 0);
        assert_eq!(kiln_realm_destroy(second), 0);
    }
}

#[cfg(feature = "realm-http")]
#[test]
fn http_callback_failure_terminates_the_parked_realm_drive() {
    use kiln_runtime::HttpDispatch;

    struct CallbackState {
        status: i32,
        calls: usize,
    }
    unsafe extern "C" fn fail_http_event(
        status: *mut c_void,
        _event: *const HttpEventView,
        _host: *mut c_void,
        _thrown: *mut c_void,
    ) -> i32 {
        // The stack status stays alive for the complete synchronous event dispatch.
        let state = unsafe { &mut *status.cast::<CallbackState>() };
        state.calls += 1;
        state.status
    }
    unsafe extern "C" fn leave_borrowed_status(_status: *mut c_void) {}

    let realm = Rc::new(NativeRealm::new().expect("create realm"));
    let mut state = CallbackState {
        status: -7,
        calls: 0,
    };
    let dispatch = NativeHttpDispatch {
        realm: Rc::downgrade(&realm),
        resource: kiln_host_abi::ResourceId::new(1),
        owner: Rc::new(HttpEventOwner {
            pointer: (&raw mut state).cast(),
            run: fail_http_event,
            drop: leave_borrowed_status,
        }),
        whole_requests: false,
    };
    let event = kiln_runtime::HttpEvent {
        // A head event reaches the language without needing a real upgrade transport.
        kind: kiln_runtime::HttpEventKind::ResponseHead,
        id: 1,
        write_fence: 0,
        keep_alive: false,
        version_minor: 1,
        status: 101,
        method: &[],
        target: &[],
        reason: &[],
        headers: &[],
        data: &[],
        message: &[],
    };
    assert_eq!(dispatch.event(&event), -7);
    // Another connection's callback cannot replace the first language failure.
    state.status = -2;
    assert_eq!(dispatch.event(&event), -7);
    assert_eq!(
        state.calls, 1,
        "another callback must not overwrite the exception"
    );
    assert!(matches!(drive_one_ready_turn(&realm), DriveStep::Done(-7)));
}
