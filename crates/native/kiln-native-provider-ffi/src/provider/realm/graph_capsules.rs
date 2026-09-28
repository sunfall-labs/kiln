//! Process-wide, opaque storage for a paused actor graph handoff.
//!
//! The archive stores only pointer bits. C owns the capsule allocation and may dereference it
//! on the sender lane while prepared, or on the receiver lane after detach. The coordinator
//! must keep both realms pair-claimed throughout that transition. No JavaScript operation can
//! call these exports, and no pointer is put in a scheduler command or actor message.

use super::{INVALID_ARGUMENT, RUNTIME_ERROR, active_realm_id};
use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Prepared,
    Detached,
}

#[derive(Clone, Copy, Debug)]
struct Capsule {
    sender: u64,
    receiver: u64,
    request: u64,
    pointer_bits: usize,
    phase: Phase,
}

#[derive(Default)]
struct CapsuleRegistry {
    by_reservation: HashMap<u64, Capsule>,
}

impl CapsuleRegistry {
    fn stage(
        &mut self,
        reservation: u64,
        sender: u64,
        receiver: u64,
        request: u64,
        pointer_bits: usize,
    ) -> i32 {
        if reservation == 0
            || sender == 0
            || receiver == 0
            || sender == receiver
            || request == 0
            || pointer_bits == 0
            || self.by_reservation.contains_key(&reservation)
            || self.by_reservation.values().any(|entry| {
                (entry.sender == sender && entry.request == request)
                    || entry.pointer_bits == pointer_bits
            })
        {
            return INVALID_ARGUMENT;
        }
        // This is the only operation that may grow the registry. It runs before sender unlink.
        if self.by_reservation.try_reserve(1).is_err() {
            return RUNTIME_ERROR;
        }
        self.by_reservation.insert(
            reservation,
            Capsule {
                sender,
                receiver,
                request,
                pointer_bits,
                phase: Phase::Prepared,
            },
        );
        0
    }

    fn detached(&mut self, reservation: u64, sender: u64, request: u64) -> i32 {
        let Some(entry) = self.by_reservation.get_mut(&reservation) else {
            return INVALID_ARGUMENT;
        };
        if entry.sender != sender || entry.request != request || entry.phase != Phase::Prepared {
            return INVALID_ARGUMENT;
        }
        // No allocation occurs after C has unlinked the sender chunk.
        entry.phase = Phase::Detached;
        0
    }

    fn cancel(&mut self, reservation: u64, sender: u64, request: u64) -> i32 {
        let Some(entry) = self.by_reservation.get(&reservation) else {
            return INVALID_ARGUMENT;
        };
        if entry.sender != sender || entry.request != request || entry.phase != Phase::Prepared {
            return INVALID_ARGUMENT;
        }
        self.by_reservation.remove(&reservation);
        0
    }

    fn take(
        &mut self,
        reservation: u64,
        sender: u64,
        receiver: u64,
        request: u64,
    ) -> Option<usize> {
        let entry = self.by_reservation.get(&reservation)?;
        if entry.sender != sender
            || entry.receiver != receiver
            || entry.request != request
            || entry.phase != Phase::Detached
        {
            return None;
        }
        // Removal neither allocates nor dereferences the capsule. The caller now owns it.
        self.by_reservation
            .remove(&reservation)
            .map(|entry| entry.pointer_bits)
    }
}

static GRAPH_CAPSULES: OnceLock<Mutex<CapsuleRegistry>> = OnceLock::new();

fn capsules() -> MutexGuard<'static, CapsuleRegistry> {
    GRAPH_CAPSULES
        .get_or_init(|| Mutex::new(CapsuleRegistry::default()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Stage an already certified C capsule while its sender still owns the chunk. The capsule
/// pointer remains opaque to Rust; active realm TLS, not a caller-supplied sender ID, binds it
/// to the owning lane. The host publication lease must be held before a later detach.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_realm_graph_capsule_stage(
    reservation: u64,
    receiver: u64,
    request: u64,
    capsule: *mut c_void,
) -> i32 {
    capsules().stage(
        reservation,
        active_realm_id(),
        receiver,
        request,
        capsule as usize,
    )
}

/// Advance a staged capsule after C has unlinked its chunk. The coordinator must treat a
/// nonzero result as a fatal transaction invariant failure: rollback is impossible now.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_realm_graph_capsule_detached(reservation: u64, request: u64) -> i32 {
    capsules().detached(reservation, active_realm_id(), request)
}

/// Cancel before unlink; C still owns and must free the capsule after this succeeds.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_realm_graph_capsule_cancel(reservation: u64, request: u64) -> i32 {
    capsules().cancel(reservation, active_realm_id(), request)
}

/// Give an already detached capsule to its exact receiver. The caller must remain inside the
/// pair claim and adopt it into the pre-reserved native inbox before either realm resumes.
///
/// # Safety
/// `out` must be a writable pointer slot. On failure it is set to null when non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_graph_capsule_take(
    reservation: u64,
    sender: u64,
    request: u64,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller provides a writable slot as documented.
    unsafe { out.write(std::ptr::null_mut()) };
    let Some(pointer_bits) = capsules().take(reservation, sender, active_realm_id(), request)
    else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: the same caller-owned output slot was validated above. Rust never dereferences it.
    unsafe { out.write(pointer_bits as *mut c_void) };
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{kiln_realm_create, kiln_realm_destroy, kiln_realm_enter};

    #[repr(C)]
    struct RealmProbe {
        reservation: u64,
        request: u64,
        sender: u64,
        receiver: u64,
    }

    unsafe extern "C" fn stage_on_sender(context: *mut c_void) -> i32 {
        // SAFETY: the test passes this live stack record for the duration of `realm_enter`.
        let probe = unsafe { &mut *context.cast::<RealmProbe>() };
        kiln_realm_graph_capsule_stage(
            probe.reservation,
            probe.receiver,
            probe.request,
            0x1234usize as *mut c_void,
        )
    }

    unsafe extern "C" fn refuse_early_take(context: *mut c_void) -> i32 {
        // SAFETY: the test passes this live stack record for the duration of `realm_enter`.
        let probe = unsafe { &mut *context.cast::<RealmProbe>() };
        let mut pointer = 0x9999usize as *mut c_void;
        // SAFETY: `pointer` is a writable local output slot.
        let status = unsafe {
            kiln_realm_graph_capsule_take(
                probe.reservation,
                probe.sender,
                probe.request,
                &mut pointer,
            )
        };
        if status == INVALID_ARGUMENT && pointer.is_null() {
            0
        } else {
            RUNTIME_ERROR
        }
    }

    unsafe extern "C" fn detach_on_sender(context: *mut c_void) -> i32 {
        // SAFETY: the test passes this live stack record for the duration of `realm_enter`.
        let probe = unsafe { &mut *context.cast::<RealmProbe>() };
        kiln_realm_graph_capsule_detached(probe.reservation, probe.request)
    }

    unsafe extern "C" fn take_on_receiver(context: *mut c_void) -> i32 {
        // SAFETY: the test passes this live stack record for the duration of `realm_enter`.
        let probe = unsafe { &mut *context.cast::<RealmProbe>() };
        let mut pointer = std::ptr::null_mut();
        // SAFETY: `pointer` is a writable local output slot.
        let status = unsafe {
            kiln_realm_graph_capsule_take(
                probe.reservation,
                probe.sender,
                probe.request,
                &mut pointer,
            )
        };
        if status == 0 && pointer as usize == 0x1234 {
            0
        } else {
            RUNTIME_ERROR
        }
    }

    #[test]
    fn graph_capsule_registry_gates_detach_and_exact_receiver_take() {
        let mut registry = CapsuleRegistry::default();
        assert_eq!(registry.stage(17, 3, 8, 11, 0x1234), 0);
        assert_eq!(registry.stage(17, 3, 8, 11, 0x2345), INVALID_ARGUMENT);
        assert_eq!(registry.stage(18, 3, 8, 11, 0x2345), INVALID_ARGUMENT);
        assert_eq!(registry.stage(18, 4, 8, 12, 0x1234), INVALID_ARGUMENT);
        assert_eq!(registry.take(17, 3, 8, 11), None);
        assert_eq!(registry.detached(17, 4, 11), INVALID_ARGUMENT);
        assert_eq!(registry.detached(17, 3, 12), INVALID_ARGUMENT);
        assert_eq!(registry.detached(17, 3, 11), 0);
        assert_eq!(registry.detached(17, 3, 11), INVALID_ARGUMENT);
        assert_eq!(registry.cancel(17, 3, 11), INVALID_ARGUMENT);
        assert_eq!(registry.take(17, 4, 8, 11), None);
        assert_eq!(registry.take(17, 3, 7, 11), None);
        assert_eq!(registry.take(17, 3, 8, 12), None);
        assert_eq!(registry.take(17, 3, 8, 11), Some(0x1234));
        assert_eq!(registry.take(17, 3, 8, 11), None);
        assert_eq!(registry.stage(18, 3, 8, 11, 0x2345), 0);
        assert_eq!(registry.cancel(18, 3, 11), 0);
        assert_eq!(registry.detached(18, 3, 11), INVALID_ARGUMENT);
    }

    #[test]
    fn graph_capsule_exports_bind_to_active_realm() {
        let mut probe = RealmProbe {
            reservation: 17_000_000_000_000_001,
            request: 47,
            sender: 0,
            receiver: 0,
        };
        // SAFETY: both output slots are valid and the created realms stay on this test thread.
        assert_eq!(unsafe { kiln_realm_create(&mut probe.sender) }, 0);
        assert_eq!(unsafe { kiln_realm_create(&mut probe.receiver) }, 0);
        assert_eq!(
            kiln_realm_graph_capsule_stage(
                probe.reservation,
                probe.receiver,
                probe.request,
                0x1234usize as *mut c_void,
            ),
            INVALID_ARGUMENT
        );
        let context = (&raw mut probe).cast::<c_void>();
        // SAFETY: callbacks and their context remain live through each synchronous entry.
        assert_eq!(
            unsafe { kiln_realm_enter(probe.sender, Some(stage_on_sender), context) },
            0
        );
        assert_eq!(
            unsafe { kiln_realm_enter(probe.receiver, Some(refuse_early_take), context) },
            0
        );
        assert_eq!(
            unsafe { kiln_realm_enter(probe.sender, Some(detach_on_sender), context) },
            0
        );
        assert_eq!(
            unsafe { kiln_realm_enter(probe.receiver, Some(take_on_receiver), context) },
            0
        );
        assert_eq!(kiln_realm_destroy(probe.sender), 0);
        assert_eq!(kiln_realm_destroy(probe.receiver), 0);
    }
}
