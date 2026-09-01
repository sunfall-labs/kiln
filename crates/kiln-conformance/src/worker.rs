//! Reusable infrastructure for one persistent interpreter worker.
//!
//! A worker owns scheduler infrastructure only. Each invocation still constructs a fresh VM,
//! language realm, global object, host capture, and value graph, then reclaims that graph before
//! returning. Keeping this owner in the conformance layer prevents the daemon from learning about
//! interpreter internals while making the lifetime promised by `docs/design.md` explicit.

use std::rc::Rc;

use kiln_interpreter::{
    ExecutionError, ExecutionLimits, execute_reclaiming_in_runtime_with_strictness,
};
use kiln_ir::{Program, Strictness};
use kiln_runtime::{CaptureHost, RealmRuntime, RuntimeBuildError};

/// Scheduler infrastructure reused by one resident interpreter thread.
///
/// This type is intentionally not `Send`: [`RealmRuntime`] owns a current-thread executor. Create
/// it inside the thread that will execute cases rather than constructing it in a coordinator and
/// moving it across threads.
#[derive(Debug)]
pub struct InterpreterWorker {
    runtime: RealmRuntime,
    reusable: bool,
    completed_executions: u64,
}

impl InterpreterWorker {
    /// Load the deterministic runtime used by every case assigned to this worker.
    pub fn new() -> Result<Self, RuntimeBuildError> {
        RealmRuntime::new().map(|runtime| Self {
            runtime,
            reusable: true,
            completed_executions: 0,
        })
    }

    /// Whether this worker can safely accept another case.
    ///
    /// A panic while the interpreter owns the runtime leaves this false. Coordinators should
    /// discard that worker before accepting another case; ordinary JavaScript exceptions do not
    /// poison it.
    pub fn is_reusable(&self) -> bool {
        self.reusable && !self.runtime.is_shutdown()
    }

    /// Number of fresh-VM executions completed on this scheduler instance.
    pub fn completed_executions(&self) -> u64 {
        self.completed_executions
    }

    pub(crate) fn recover(&mut self) -> Result<(), RuntimeBuildError> {
        if self.is_reusable() {
            return Ok(());
        }
        *self = Self::new()?;
        Ok(())
    }

    pub(crate) fn execute_reclaiming(
        &mut self,
        program: Rc<Program>,
        strictness: Rc<Strictness>,
        host: &mut CaptureHost,
        limits: ExecutionLimits,
    ) -> Result<u64, ExecutionError> {
        assert!(
            self.is_reusable(),
            "a poisoned interpreter worker must be replaced before reuse"
        );
        self.reusable = false;
        let outcome = execute_reclaiming_in_runtime_with_strictness(
            program,
            strictness,
            host,
            limits,
            &mut self.runtime,
        );
        self.reusable = true;
        self.completed_executions = self.completed_executions.saturating_add(1);
        outcome
    }
}
