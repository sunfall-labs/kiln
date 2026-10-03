//! Dense dominator sets shared by graph optimization and verification.
//!
//! The fixed-point equations are unchanged from the HashSet implementation, but each set is a
//! compact bit vector. Intersections therefore scan contiguous words and do not allocate a new
//! hash table for every predecessor on every iteration.

use crate::graph::{BlockId, FunctionGraph};
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub(crate) struct Dominators {
    indices: HashMap<BlockId, usize>,
    sets: Vec<Vec<u64>>,
}

impl Dominators {
    pub(crate) fn contains(&self, dominator: BlockId, block: BlockId) -> bool {
        let (Some(&dominator), Some(&block)) =
            (self.indices.get(&dominator), self.indices.get(&block))
        else {
            return false;
        };
        self.sets[block][dominator / 64] & (1 << (dominator % 64)) != 0
    }

    pub(crate) fn into_hash_map(self, ids: &[BlockId]) -> HashMap<BlockId, HashSet<BlockId>> {
        let mut result = HashMap::with_capacity(ids.len());
        for (block_index, &block) in ids.iter().enumerate() {
            let mut set = HashSet::new();
            for (word_index, &word) in self.sets[block_index].iter().enumerate() {
                let mut remaining = word;
                while remaining != 0 {
                    let bit = remaining.trailing_zeros() as usize;
                    let dominator_index = word_index * 64 + bit;
                    if let Some(&dominator) = ids.get(dominator_index) {
                        set.insert(dominator);
                    }
                    remaining &= remaining - 1;
                }
            }
            result.insert(block, set);
        }
        result
    }
}

/// Compute dominators with the graph optimizer's reachability rules or the verifier's historical
/// all-block rules. The verifier intentionally includes unreachable blocks in its equations; the
/// optimizer gives each unreachable block only itself as a dominator.
pub(crate) fn compute(graph: &FunctionGraph, include_unreachable_in_equations: bool) -> Dominators {
    let ids: Vec<BlockId> = graph.blocks.iter().map(|block| block.id).collect();
    let indices: HashMap<BlockId, usize> = ids
        .iter()
        .enumerate()
        .map(|(index, &id)| (id, index))
        .collect();
    let count = ids.len();
    let words = count.div_ceil(64);
    let mut sets = vec![vec![0; words]; count];
    if count == 0 {
        return Dominators { indices, sets };
    }

    let entry = indices.get(&graph.entry_block).copied();
    let reachable = if include_unreachable_in_equations {
        HashSet::new()
    } else {
        let mut reachable = HashSet::new();
        let mut worklist = vec![graph.entry_block];
        while let Some(block_id) = worklist.pop() {
            if !reachable.insert(block_id) {
                continue;
            }
            if let Some(&index) = indices.get(&block_id) {
                worklist.extend(graph.blocks[index].successors.iter().copied());
            }
        }
        reachable
    };

    let mut universe = vec![u64::MAX; words];
    if let Some(last) = universe.last_mut()
        && count % 64 != 0
    {
        *last = (1 << (count % 64)) - 1;
    }

    for index in 0..count {
        if Some(index) == entry {
            set_bit(&mut sets[index], index);
        } else if include_unreachable_in_equations || reachable.contains(&ids[index]) {
            sets[index].copy_from_slice(&universe);
        } else {
            set_bit(&mut sets[index], index);
        }
    }

    let mut changed = true;
    let mut next = vec![0; words];
    while changed {
        changed = false;
        for (block_index, block) in graph.blocks.iter().enumerate() {
            if Some(block_index) == entry
                || (!include_unreachable_in_equations && !reachable.contains(&block.id))
            {
                continue;
            }
            let mut first_predecessor = true;
            if include_unreachable_in_equations {
                // Match the verifier's former initialization: a block with no predecessors keeps
                // the all-block set (and blocks with predecessors intersect from that universe).
                next.copy_from_slice(&universe);
            } else {
                next.fill(0);
            }
            for predecessor in &block.predecessors {
                if !include_unreachable_in_equations && !reachable.contains(predecessor) {
                    continue;
                }
                let Some(&predecessor_index) = indices.get(predecessor) else {
                    continue;
                };
                if first_predecessor {
                    next.copy_from_slice(&sets[predecessor_index]);
                    first_predecessor = false;
                } else {
                    for (word, predecessor_word) in next.iter_mut().zip(&sets[predecessor_index]) {
                        *word &= *predecessor_word;
                    }
                }
            }
            set_bit(&mut next, block_index);
            if next != sets[block_index] {
                sets[block_index].copy_from_slice(&next);
                changed = true;
            }
        }
    }

    Dominators { indices, sets }
}

fn set_bit(set: &mut [u64], index: usize) {
    set[index / 64] |= 1 << (index % 64);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{BasicBlock, FunctionGraph, Terminator};

    fn reference(
        graph: &FunctionGraph,
        include_unreachable_in_equations: bool,
    ) -> HashMap<BlockId, HashSet<BlockId>> {
        let ids: HashSet<BlockId> = graph.blocks.iter().map(|block| block.id).collect();
        let reachable = if include_unreachable_in_equations {
            ids.clone()
        } else {
            let mut reachable = HashSet::new();
            let mut worklist = vec![graph.entry_block];
            while let Some(id) = worklist.pop() {
                if reachable.insert(id)
                    && let Some(block) = graph.block(id)
                {
                    worklist.extend(block.successors.iter().copied());
                }
            }
            reachable
        };
        let mut result = HashMap::new();
        for &id in &ids {
            if id == graph.entry_block {
                result.insert(id, HashSet::from([id]));
            } else if include_unreachable_in_equations || reachable.contains(&id) {
                result.insert(id, ids.clone());
            } else {
                result.insert(id, HashSet::from([id]));
            }
        }
        let mut changed = true;
        while changed {
            changed = false;
            for block in &graph.blocks {
                if block.id == graph.entry_block
                    || (!include_unreachable_in_equations && !reachable.contains(&block.id))
                {
                    continue;
                }
                let mut next: Option<HashSet<BlockId>> =
                    include_unreachable_in_equations.then(|| ids.clone());
                for predecessor in &block.predecessors {
                    if !include_unreachable_in_equations && !reachable.contains(predecessor) {
                        continue;
                    }
                    if let Some(previous) = result.get(predecessor) {
                        next = Some(match next {
                            Some(current) => current.intersection(previous).copied().collect(),
                            None => previous.clone(),
                        });
                    }
                }
                let mut next = next.unwrap_or_default();
                next.insert(block.id);
                if result.get(&block.id) != Some(&next) {
                    result.insert(block.id, next);
                    changed = true;
                }
            }
        }
        result
    }

    fn graph_from_edges(count: usize, edges: &[(usize, usize)]) -> FunctionGraph {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.entry_block = BlockId(0);
        graph.blocks = (0..count)
            .map(|index| BasicBlock {
                id: BlockId((index * 7 + 3) as u32),
                operations: Vec::new(),
                terminator: Terminator::Unreachable,
                predecessors: Vec::new(),
                successors: Vec::new(),
                exception_handler: None,
            })
            .collect();
        // Give the permuted IDs a deterministic, separately chosen entry.
        graph.entry_block = graph.blocks[0].id;
        for &(from, to) in edges {
            let from_id = graph.blocks[from].id;
            let to_id = graph.blocks[to].id;
            graph.blocks[from].successors.push(to_id);
            graph.blocks[to].predecessors.push(from_id);
        }
        graph
    }

    #[test]
    fn dense_sets_match_hashset_equations_for_reachable_and_unreachable_cfgs() {
        // Includes diamonds, backedges, disconnected components, and predecessor-free blocks;
        // permuted BlockIds ensure bit positions do not rely on identifier density.
        let mut seed = 0x2a5a_17d3_u32;
        for count in 1..48 {
            let mut edges = Vec::new();
            for from in 0..count {
                for to in 0..count {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    if from != to && seed % 13 == 0 {
                        edges.push((from, to));
                    }
                }
            }
            // Ensure a rooted chain among part of the graph and leave the rest potentially
            // unreachable so both policies are exercised on each generated graph.
            for index in 1..count.min(8) {
                edges.push((index - 1, index));
            }
            let graph = graph_from_edges(count, &edges);
            let ids: Vec<_> = graph.blocks.iter().map(|block| block.id).collect();
            for include_unreachable in [false, true] {
                let actual = compute(&graph, include_unreachable).into_hash_map(&ids);
                assert_eq!(
                    actual,
                    reference(&graph, include_unreachable),
                    "count={count}, include_unreachable={include_unreachable}"
                );
            }
        }
    }

    #[test]
    fn large_cfg_keeps_dominator_queries_compact() {
        let count = 4096;
        let edges: Vec<_> = (1..count).map(|index| (index - 1, index)).collect();
        let graph = graph_from_edges(count, &edges);
        let dominators = compute(&graph, false);
        assert!(dominators.contains(graph.blocks[0].id, graph.blocks[count - 1].id));
        assert!(dominators.contains(graph.blocks[count / 2].id, graph.blocks[count - 1].id));
        assert!(!dominators.contains(graph.blocks[count - 1].id, graph.blocks[0].id));
        assert!(dominators.contains(graph.blocks[count - 1].id, graph.blocks[count - 1].id));
    }

    #[test]
    fn verifier_preserves_all_block_set_for_a_predecessor_free_block() {
        let graph = graph_from_edges(3, &[(0, 1)]);
        let dominators = compute(&graph, true);
        assert!(dominators.contains(graph.blocks[0].id, graph.blocks[2].id));
        assert!(dominators.contains(graph.blocks[1].id, graph.blocks[2].id));
        assert!(dominators.contains(graph.blocks[2].id, graph.blocks[2].id));
    }
}
