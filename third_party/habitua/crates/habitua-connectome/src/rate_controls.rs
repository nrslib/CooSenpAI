use serde::{Deserialize, Serialize};

use crate::rate::{RateGraph, RateModelError, RateOutgoingEdge};
use crate::rate_learning::{FeedbackShuffleResult, FeedbackStore};

/// Statistics needed to verify a degree-preserving wiring control.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RateControlStats {
    /// Number of random pair attempts.
    pub attempts: usize,
    /// Number of accepted edge swaps.
    pub successes: usize,
    /// Number of rejected attempts.
    pub rejected: usize,
    /// Number of edge slots whose target or contact count changed.
    pub changed_edge_count: usize,
    /// Fraction of directed pairs retained from the original graph.
    pub original_edge_overlap: f64,
}

/// A shuffled graph and its mechanical verification statistics.
#[derive(Clone, Debug, PartialEq)]
pub struct RateControlResult {
    /// Resulting graph.
    pub graph: RateGraph,
    /// Construction statistics.
    pub stats: RateControlStats,
}

/// Creates a directed degree-preserving wiring shuffle.
pub fn shuffle_wiring(
    graph: &RateGraph,
    seed: u64,
    requested_swaps: usize,
) -> Result<RateControlResult, RateModelError> {
    let mut edges = graph.outgoing_edges.clone();
    let mut changed_edge_ids = Vec::new();
    let mut random = seed.max(1);
    let mut stats = RateControlStats {
        attempts: 0,
        successes: 0,
        rejected: 0,
        changed_edge_count: 0,
        original_edge_overlap: 1.0,
    };
    let max_attempts = requested_swaps.saturating_mul(32).max(requested_swaps);
    while stats.successes < requested_swaps && stats.attempts < max_attempts {
        stats.attempts += 1;
        if edges.len() < 2 {
            stats.rejected += 1;
            break;
        }
        let first = (next_random(&mut random) as usize) % edges.len();
        let mut second = (next_random(&mut random) as usize) % edges.len();
        if first == second {
            second = (second + 1) % edges.len();
        }
        let source_first = source_for_edge(graph, first);
        let source_second = source_for_edge(graph, second);
        if source_first == source_second
            || graph.source_sign[source_first as usize] != graph.source_sign[source_second as usize]
        {
            stats.rejected += 1;
            continue;
        }
        let old_first = edges[first];
        let old_second = edges[second];
        let new_first = (source_first, old_second.target);
        let new_second = (source_second, old_first.target);
        let valid = new_first.0 != new_first.1
            && new_second.0 != new_second.1
            && !pair_is_occupied(
                graph,
                &edges,
                &changed_edge_ids,
                new_first.0,
                new_first.1,
                first,
                second,
            )
            && !pair_is_occupied(
                graph,
                &edges,
                &changed_edge_ids,
                new_second.0,
                new_second.1,
                first,
                second,
            )
            && new_first != new_second;
        if !valid {
            stats.rejected += 1;
            continue;
        }
        edges[first] = RateOutgoingEdge {
            target: old_second.target,
            contact_count: old_second.contact_count,
        };
        edges[second] = RateOutgoingEdge {
            target: old_first.target,
            contact_count: old_first.contact_count,
        };
        if !changed_edge_ids.contains(&first) {
            changed_edge_ids.push(first);
        }
        if !changed_edge_ids.contains(&second) {
            changed_edge_ids.push(second);
        }
        stats.successes += 1;
    }
    stats.rejected = stats.attempts.saturating_sub(stats.successes);
    stats.changed_edge_count = edges
        .iter()
        .zip(&graph.outgoing_edges)
        .filter(|(actual, original)| actual != original)
        .count();
    let overlap = edges
        .iter()
        .zip(&graph.outgoing_edges)
        .filter(|(actual, original)| actual.target == original.target)
        .count();
    stats.original_edge_overlap = overlap as f64 / edges.len().max(1) as f64;
    Ok(RateControlResult {
        graph: rebuild_graph(graph, edges)?,
        stats,
    })
}

/// Shuffles contact counts within each receiver and transmitter-sign group.
pub fn shuffle_weights(graph: &RateGraph, seed: u64) -> Result<RateControlResult, RateModelError> {
    let mut grouped_edges = Vec::with_capacity(graph.edge_count());
    for (edge_id, edge) in graph.outgoing_edges.iter().enumerate() {
        let source = source_for_edge(graph, edge_id);
        grouped_edges.push(WeightKey {
            target: edge.target,
            sign: graph.source_sign[source as usize],
            edge_id: edge_id as u32,
        });
    }
    grouped_edges.sort_unstable_by_key(|key| (key.target, key.sign, key.edge_id));
    let mut edges = graph.outgoing_edges.clone();
    let mut random = seed.max(1);
    let mut start = 0;
    while start < grouped_edges.len() {
        let key = grouped_edges[start];
        let mut end = start + 1;
        while end < grouped_edges.len()
            && grouped_edges[end].target == key.target
            && grouped_edges[end].sign == key.sign
        {
            end += 1;
        }
        for index in (start + 1..end).rev() {
            let swap = start + (next_random(&mut random) as usize) % (index - start + 1);
            let left = grouped_edges[index].edge_id as usize;
            let right = grouped_edges[swap].edge_id as usize;
            let left_contact_count = edges[left].contact_count;
            edges[left].contact_count = edges[right].contact_count;
            edges[right].contact_count = left_contact_count;
        }
        start = end;
    }
    let changed_edge_count = edges
        .iter()
        .zip(&graph.outgoing_edges)
        .filter(|(actual, original)| actual != original)
        .count();
    Ok(RateControlResult {
        graph: rebuild_graph(graph, edges)?,
        stats: RateControlStats {
            changed_edge_count,
            ..RateControlStats::default()
        },
    })
}

#[derive(Clone, Copy)]
struct WeightKey {
    target: u32,
    sign: i8,
    edge_id: u32,
}

fn pair_is_occupied(
    graph: &RateGraph,
    edges: &[RateOutgoingEdge],
    changed_edge_ids: &[usize],
    source: u32,
    target: u32,
    first_excluded: usize,
    second_excluded: usize,
) -> bool {
    for edge_id in changed_edge_ids {
        if *edge_id == first_excluded || *edge_id == second_excluded {
            continue;
        }
        if source_for_edge(graph, *edge_id) == source && edges[*edge_id].target == target {
            return true;
        }
    }
    graph
        .outgoing_range(source as usize)
        .expect("validated graph")
        .any(|edge_id| {
            edge_id != first_excluded
                && edge_id != second_excluded
                && !changed_edge_ids.contains(&edge_id)
                && graph.outgoing_edges[edge_id].target == target
        })
}

/// Applies the reward-shuffled control while preserving event signs, strengths, and times.
pub fn shuffle_feedback(store: &FeedbackStore, seed: u64) -> FeedbackShuffleResult {
    store.shuffled(seed)
}

fn rebuild_graph(
    graph: &RateGraph,
    outgoing_edges: Vec<RateOutgoingEdge>,
) -> Result<RateGraph, RateModelError> {
    let mut incoming = outgoing_edges
        .iter()
        .enumerate()
        .map(|(edge_id, edge)| {
            let source = source_for_edge(graph, edge_id);
            (edge.target, source, edge_id as u32)
        })
        .collect::<Vec<_>>();
    incoming.sort_unstable();
    let mut incoming_offsets = vec![0_u64; graph.neuron_count() + 1];
    for (target, _, _) in &incoming {
        incoming_offsets[*target as usize + 1] += 1;
    }
    for index in 1..incoming_offsets.len() {
        incoming_offsets[index] += incoming_offsets[index - 1];
    }
    let incoming_edges = incoming
        .into_iter()
        .map(|(_, source, edge_id)| crate::rate::RateIncomingEdge { source, edge_id })
        .collect();
    RateGraph::new(
        graph.root_ids.clone(),
        graph.outgoing_offsets.clone(),
        outgoing_edges,
        incoming_offsets,
        incoming_edges,
        graph.source_sign.clone(),
        graph.neuron_metadata.clone(),
        graph.populations.clone(),
    )
}

fn source_for_edge(graph: &RateGraph, edge_id: usize) -> u32 {
    graph
        .outgoing_offsets
        .partition_point(|offset| *offset <= edge_id as u64)
        .saturating_sub(1) as u32
}

fn next_random(state: &mut u64) -> u64 {
    let mut value = *state;
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    *state = value;
    value
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::rate::{RateIncomingEdge, RateNeuronMetadata};

    fn graph() -> RateGraph {
        let metadata = (0..4)
            .map(|body_id| RateNeuronMetadata {
                body_id,
                type_name: Some(format!("T{body_id}")),
                class: None,
                superclass: None,
                subclass: None,
                soma_side: None,
                root_side: None,
                assigned_ol_hex1: None,
                assigned_ol_hex2: None,
                selected_neurotransmitter: None,
                neurotransmitter_source: "test".to_owned(),
                source_sign: 1,
            })
            .collect();
        RateGraph::new(
            vec![0, 1, 2, 3],
            vec![0, 2, 4, 5, 6],
            vec![
                RateOutgoingEdge {
                    target: 1,
                    contact_count: 2,
                },
                RateOutgoingEdge {
                    target: 2,
                    contact_count: 3,
                },
                RateOutgoingEdge {
                    target: 2,
                    contact_count: 5,
                },
                RateOutgoingEdge {
                    target: 3,
                    contact_count: 7,
                },
                RateOutgoingEdge {
                    target: 0,
                    contact_count: 11,
                },
                RateOutgoingEdge {
                    target: 0,
                    contact_count: 13,
                },
            ],
            vec![0, 2, 3, 5, 6],
            vec![
                RateIncomingEdge {
                    source: 2,
                    edge_id: 4,
                },
                RateIncomingEdge {
                    source: 3,
                    edge_id: 5,
                },
                RateIncomingEdge {
                    source: 0,
                    edge_id: 0,
                },
                RateIncomingEdge {
                    source: 0,
                    edge_id: 1,
                },
                RateIncomingEdge {
                    source: 1,
                    edge_id: 2,
                },
                RateIncomingEdge {
                    source: 1,
                    edge_id: 3,
                },
            ],
            vec![1; 4],
            metadata,
            BTreeMap::new(),
        )
        .expect("test graph")
    }

    #[test]
    fn wiring_shuffle_preserves_degrees_and_receiver_contact_totals() {
        let original = graph();
        let shuffled = shuffle_wiring(&original, 11, 1).expect("shuffle");
        assert_eq!(shuffled.graph.neuron_count(), original.neuron_count());
        assert_eq!(shuffled.graph.edge_count(), original.edge_count());
        assert_eq!(
            shuffled.graph.incoming_contact_totals,
            original.incoming_contact_totals
        );
        assert!(shuffled.stats.successes <= 1);
    }

    #[test]
    fn weight_shuffle_preserves_target_sign_group_sums() {
        let original = graph();
        let shuffled = shuffle_weights(&original, 19).expect("shuffle");
        let original_sums = target_sums(&original);
        let shuffled_sums = target_sums(&shuffled.graph);
        assert_eq!(original_sums, shuffled_sums);
    }

    fn target_sums(graph: &RateGraph) -> BTreeMap<u32, u64> {
        graph
            .outgoing_edges
            .iter()
            .fold(BTreeMap::new(), |mut sums, edge| {
                *sums.entry(edge.target).or_default() += edge.contact_count;
                sums
            })
    }
}
