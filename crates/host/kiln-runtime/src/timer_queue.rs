//! The realm's armed timers, in the order they run.
//!
//! The order is `(deadline, sequence, id)` and nothing else: the earliest deadline first, and
//! among equal deadlines the timer armed first. That order is observable, so this structure may
//! change how it is found but never what is found.
//!
//! A single binary heap found it in `O(log n)` with a cache miss per level, which is what a
//! hundred thousand sleeping tasks paid on every wake. But a realm arms timers in order: a timer
//! armed with delay `d` at virtual time `now` is due at `now + d`, `now` never moves backwards,
//! and the sequence only grows -- so all the timers armed with one delay are already sorted by the
//! time they are armed. Node keeps one list per duration for exactly this reason. Here each delay
//! has a FIFO run, and a small heap holds only the front of each run: finding the next timer is a
//! heap of distinct delays (usually a handful), and arming or taking one is a push or a pop at
//! the end of a `VecDeque`.
//!
//! The one way a run could fall out of order is an interval re-armed from a deadline that ran
//! late on a wall clock (`deadline + interval` can precede a timer armed at the later `now` with
//! the same delay). Any entry that would sort before the back of its run goes to `stragglers`, an
//! ordinary heap, and the next timer is the lesser of the two fronts -- so the order is exact
//! whatever the clock did.

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::hash::{BuildHasherDefault, Hasher};
use std::time::Duration;

use crate::TimerId;

/// A multiplicative hash for the realm's small integer keys (timer identities, delays).
///
/// Timer identities are sequential and never adversarial -- the realm hands them out -- so the
/// keyed SipHash `HashSet` defaulted to was paying for a property nothing here needs, twice per
/// timer. This is the FxHash step: rotate, xor, multiply by an odd constant.
#[derive(Default, Clone, Copy)]
pub(crate) struct IdHasher(u64);

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl IdHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for IdHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u32(&mut self, value: u32) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.add(value);
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.add(value as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub(crate) type IdBuild = BuildHasherDefault<IdHasher>;

/// The hash for the sets of live timer identities, which are handed out in sequence and retired
/// roughly in the order they were armed.
///
/// A hashbrown table picks the bucket from a hash's low bits and keeps its top seven as the tag
/// it compares first. Mixing the whole identity sends consecutive timers to random buckets, and
/// with a hundred thousand armed that is a cache miss on every arm and every expiry. This keeps
/// the identity itself in the low bits, so consecutive timers land in consecutive buckets and an
/// arm touches the line the previous arm touched, and puts mixed bits in the top seven so the tag
/// still tells neighbours apart.
#[derive(Default, Clone, Copy)]
pub(crate) struct SequenceHasher(u64);

const TAG_BITS: u64 = !((1u64 << 57) - 1);

impl Hasher for SequenceHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut mixed = IdHasher(self.0);
        mixed.write(bytes);
        self.0 = mixed.0;
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }

    #[inline]
    fn finish(&self) -> u64 {
        (self.0 & !TAG_BITS) | (self.0.wrapping_mul(SEED) & TAG_BITS)
    }
}

pub(crate) type SequenceBuild = BuildHasherDefault<SequenceHasher>;

pub(crate) struct TimerEntry<T> {
    pub(crate) deadline: Duration,
    pub(crate) sequence: u64,
    pub(crate) id: TimerId,
    pub(crate) work: T,
    pub(crate) interval: Option<Duration>,
    /// The delay this entry was armed with, which is what `Realm::refresh_timer` measures from
    /// its new base: the run it sits in.
    pub(crate) delay: Duration,
}

impl<T> TimerEntry<T> {
    #[inline]
    fn key(&self) -> (Duration, u64, u64) {
        (self.deadline, self.sequence, self.id.get())
    }
}

impl<T> PartialEq for TimerEntry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl<T> Eq for TimerEntry<T> {}

impl<T> Ord for TimerEntry<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap; reverse the semantic order so the earliest, then oldest,
        // timer is returned first.
        other.key().cmp(&self.key())
    }
}

impl<T> PartialOrd for TimerEntry<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// How many emptied runs are kept for reuse. A program that sleeps on one or two delays keeps its
/// runs (and their buffers) forever; one that arms a different delay every time does not grow a
/// map of empty runs without bound.
const IDLE_RUNS: usize = 64;

pub(crate) struct TimerQueue<T> {
    runs: HashMap<Duration, VecDeque<TimerEntry<T>>, IdBuild>,
    /// One entry per non-empty run: the key of its front entry, and the run's delay.
    fronts: BinaryHeap<Reverse<((Duration, u64, u64), Duration)>>,
    stragglers: BinaryHeap<TimerEntry<T>>,
    len: usize,
}

enum Source {
    Run(Duration),
    Straggler,
}

impl<T> TimerQueue<T> {
    pub(crate) fn new() -> Self {
        Self {
            runs: HashMap::default(),
            fronts: BinaryHeap::new(),
            stragglers: BinaryHeap::new(),
            len: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Arms `entry`, which was scheduled `delay` after some reading of the clock.
    pub(crate) fn push(&mut self, delay: Duration, entry: TimerEntry<T>) {
        self.len += 1;
        let key = entry.key();
        let run = self.runs.entry(delay).or_default();
        match run.back() {
            None => {
                run.push_back(entry);
                self.fronts.push(Reverse((key, delay)));
            }
            Some(back) if back.key() <= key => run.push_back(entry),
            Some(_) => self.stragglers.push(entry),
        }
    }

    fn next_source(&self) -> Option<Source> {
        let run_front = self
            .fronts
            .peek()
            .map(|Reverse((key, delay))| (*key, *delay));
        let straggler = self.stragglers.peek().map(TimerEntry::key);
        match (run_front, straggler) {
            (None, None) => None,
            (Some((_, delay)), None) => Some(Source::Run(delay)),
            (None, Some(_)) => Some(Source::Straggler),
            (Some((run_key, delay)), Some(straggler_key)) => {
                if run_key <= straggler_key {
                    Some(Source::Run(delay))
                } else {
                    Some(Source::Straggler)
                }
            }
        }
    }

    /// The `(deadline, sequence, id)` of the timer that runs next, without finding its entry:
    /// what the drive asks on every turn, answered from the fronts alone.
    pub(crate) fn peek_key(&self) -> Option<(Duration, u64, u64)> {
        let run_front = self.fronts.peek().map(|Reverse((key, _))| *key);
        let straggler = self.stragglers.peek().map(TimerEntry::key);
        match (run_front, straggler) {
            (Some(run), Some(straggler)) => Some(run.min(straggler)),
            (run, straggler) => run.or(straggler),
        }
    }

    /// The timer that runs next.
    #[cfg(test)]
    pub(crate) fn peek(&self) -> Option<&TimerEntry<T>> {
        match self.next_source()? {
            Source::Run(delay) => self.runs.get(&delay).and_then(VecDeque::front),
            Source::Straggler => self.stragglers.peek(),
        }
    }

    /// Takes the timer that runs next.
    pub(crate) fn pop(&mut self) -> Option<TimerEntry<T>> {
        let entry = match self.next_source()? {
            Source::Straggler => self.stragglers.pop(),
            Source::Run(delay) => {
                self.fronts.pop();
                let run = self.runs.get_mut(&delay).expect("a front names a live run");
                let entry = run.pop_front();
                match run.front() {
                    Some(next) => self.fronts.push(Reverse((next.key(), delay))),
                    None => {
                        if self.runs.len() > IDLE_RUNS {
                            self.runs.remove(&delay);
                        }
                    }
                }
                entry
            }
        };
        if entry.is_some() {
            self.len -= 1;
        }
        entry
    }

    /// The entry `ahead` places behind the next timer in the next timer's own run: usually the
    /// timer that runs `ahead` turns from now, and exactly that when one delay dominates. For a
    /// cache hint, so a guess that another run interleaves costs nothing but the hint.
    pub(crate) fn upcoming(&self, ahead: usize) -> Option<&TimerEntry<T>> {
        let Reverse((_, delay)) = self.fronts.peek()?;
        self.runs.get(delay)?.get(ahead)
    }

    /// Every armed entry, in no particular order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &TimerEntry<T>> {
        self.runs.values().flatten().chain(self.stragglers.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(deadline_ms: u64, sequence: u64) -> TimerEntry<u64> {
        TimerEntry {
            deadline: Duration::from_millis(deadline_ms),
            sequence,
            id: TimerId::new(sequence + 1),
            work: sequence,
            interval: None,
            delay: Duration::ZERO,
        }
    }

    /// Whatever arrives, in whatever runs, comes out in exactly the order one heap gives.
    #[test]
    fn matches_a_single_heap_in_every_interleaving() {
        let mut queue = TimerQueue::new();
        let mut reference = BinaryHeap::new();
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut now = 0u64;
        let mut sequence = 0u64;
        let mut popped = Vec::new();
        let mut expected = Vec::new();
        for _ in 0..20_000 {
            if next() % 3 != 0 {
                let delay = [0u64, 1, 10, 10, 10, 25, 1000][(next() % 7) as usize];
                // Now and then a deadline behind the clock, as a late interval re-arm makes.
                let base = if next() % 11 == 0 {
                    now.saturating_sub(next() % 20)
                } else {
                    now
                };
                queue.push(Duration::from_millis(delay), entry(base + delay, sequence));
                reference.push(entry(base + delay, sequence));
                sequence += 1;
            } else if let Some(taken) = queue.pop() {
                now = now.max(taken.deadline.as_millis() as u64);
                popped.push(taken.work);
                expected.push(reference.pop().unwrap().work);
            }
            assert_eq!(
                queue.peek().map(|e| e.work),
                reference.peek().map(|e| e.work)
            );
            assert_eq!(queue.peek_key(), reference.peek().map(TimerEntry::key));
            assert_eq!(queue.len(), reference.len());
            assert_eq!(queue.iter().count(), reference.len());
        }
        while let Some(taken) = queue.pop() {
            popped.push(taken.work);
            expected.push(reference.pop().unwrap().work);
        }
        assert_eq!(popped, expected);
        assert!(reference.is_empty());
    }
}
