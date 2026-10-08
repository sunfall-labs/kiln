//! Which files each checker task checks, ported from Bun's `src/sema/driver` (`Plan`,
//! `PlanOptions`) at the commit `NOTICE` names. Splitting declaration files into ranges checked
//! ahead is not ported: every task checks whole files.
//!
//! A plan is a function of the program alone, not of the thread count or of time. During a step
//! the published state is read-only and each task writes to its own buffer, so the result of a
//! task is a function of the program and of the published state at the start of its step; by
//! induction over the steps, so is the output.

/// One task: the indices of its files in program order among the files to check.
pub(crate) type Task = Vec<usize>;

/// The donor's defaults. See `PlanOptions` there.
const STEP_GROWTH: usize = 8;
const WARM_UP_FILES: usize = 1 + 8 + 64;
const WARM_UP_MAX_BYTES: usize = 16 << 10;
const MIN_TASKS: usize = 128;

pub(crate) struct Plan {
    pub steps: Vec<Vec<Task>>,
}

impl Plan {
    /// `count`: the number of files to check. `bytes_of(i)`: the source size of file `i`.
    /// `cost_of(i)`: its cost estimate.
    pub fn new(
        count: usize,
        bytes_of: &dyn Fn(usize) -> usize,
        cost_of: &dyn Fn(usize) -> usize,
    ) -> Plan {
        let mut steps: Vec<Vec<Task>> = Vec::new();
        // The warm-up fills the published state with what most tasks need: steps of growing size of
        // light files, sampled from the whole program, one file per task.
        let light: Vec<usize> = (0..count)
            .filter(|&file| bytes_of(file) <= WARM_UP_MAX_BYTES)
            .collect();
        let expected = WARM_UP_FILES.min(light.len());
        let warm_up: Vec<usize> = (0..expected)
            .map(|i| light[i * light.len() / expected])
            .collect();
        let (mut tasks, mut limit) = (warm_up.iter().map(|&file| vec![file]).peekable(), 1usize);
        while tasks.peek().is_some() {
            steps.push(tasks.by_ref().take(limit).collect());
            limit = limit.saturating_mul(STEP_GROWTH);
        }
        // One step with every other file.
        let rest = (0..count).filter(|file| warm_up.binary_search(file).is_err());
        let tasks = Plan::cut(rest.collect(), cost_of);
        if !tasks.is_empty() {
            steps.push(tasks);
        }
        Plan { steps }
    }

    /// The tasks of one step for `rest`, which is in program order: about `MIN_TASKS` tasks of
    /// consecutive files, and a task of its own for each heavy file.
    pub fn cut(rest: Vec<usize>, cost_of: &dyn Fn(usize) -> usize) -> Vec<Task> {
        let total: usize = rest.iter().map(|&file| cost_of(file)).sum();
        let chunk = (total / MIN_TASKS).max(1);
        let mut chunks: Vec<Task> = Vec::new();
        // The cost of the last chunk, if it consists of light files.
        let mut cost_of_last = None;
        for file in rest {
            match cost_of_last {
                _ if cost_of(file) >= chunk => {
                    chunks.push(vec![file]);
                    cost_of_last = None;
                }
                Some(so_far) if so_far < chunk => {
                    chunks.last_mut().unwrap().push(file);
                    cost_of_last = Some(so_far + cost_of(file));
                }
                _ => {
                    chunks.push(vec![file]);
                    cost_of_last = Some(cost_of(file));
                }
            }
        }
        chunks
    }
}
