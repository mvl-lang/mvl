// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Circular import detection using depth-first search (DFS).
//!
//! Given a directed graph of module dependencies (module A imports from module B),
//! this module finds all strongly-connected components with more than one node, or
//! any self-loop — i.e., any import cycle.
//!
//! The algorithm is a standard DFS-based cycle detector:
//! - Gray: currently on the DFS stack (in-progress)
//! - Black: fully explored (no cycles found through here)
//!
//! When a gray node is encountered during DFS, we have found a back-edge, indicating
//! a cycle.  The cycle is reconstructed from the DFS stack path.

use std::collections::HashMap;

/// Detect all import cycles in the dependency graph.
///
/// `graph`: adjacency list — `graph[module]` = list of modules that `module` imports from.
///
/// Returns a list of cycles, where each cycle is the sequence of module names
/// forming the loop (the first and last element are the same module to make
/// the cycle explicit).
pub fn detect_cycles(graph: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    let mut cycles = Vec::new();
    let mut color: HashMap<&str, Color> = HashMap::new();
    let mut stack: Vec<&str> = Vec::new();

    for node in graph.keys() {
        if !color.contains_key(node.as_str()) {
            dfs(node, graph, &mut color, &mut stack, &mut cycles);
        }
    }

    cycles
}

#[derive(Clone, Copy, PartialEq)]
enum Color {
    /// Currently on the DFS stack (being explored).
    Gray,
    /// Fully explored.
    Black,
}

fn dfs<'a>(
    node: &'a str,
    graph: &'a HashMap<String, Vec<String>>,
    color: &mut HashMap<&'a str, Color>,
    stack: &mut Vec<&'a str>,
    cycles: &mut Vec<Vec<String>>,
) {
    color.insert(node, Color::Gray);
    stack.push(node);

    if let Some(neighbors) = graph.get(node) {
        for neighbor in neighbors {
            let nb = neighbor.as_str();
            match color.get(nb) {
                Some(Color::Gray) => {
                    // Back-edge found: reconstruct the cycle from the stack.
                    let cycle_start = stack.iter().position(|&n| n == nb).unwrap_or(0);
                    let mut cycle: Vec<String> =
                        stack[cycle_start..].iter().map(|s| s.to_string()).collect();
                    // Close the cycle by repeating the first element
                    cycle.push(nb.to_string());
                    // Only record the cycle if we haven't already seen this exact one.
                    let canonical = canonical_cycle(&cycle);
                    if !cycles.iter().any(|c| canonical_cycle(c) == canonical) {
                        cycles.push(cycle);
                    }
                }
                Some(Color::Black) => {} // already fully explored, no cycle through here
                None => {
                    dfs(nb, graph, color, stack, cycles);
                }
            }
        }
    }

    stack.pop();
    color.insert(node, Color::Black);
}

/// Rotate the cycle so the lexicographically-smallest element is first,
/// providing a canonical form for deduplication.
fn canonical_cycle(cycle: &[String]) -> Vec<String> {
    if cycle.len() <= 1 {
        return cycle.to_vec();
    }
    // The last element is a repeat of the first (closing the cycle); exclude it.
    let body = &cycle[..cycle.len() - 1];
    let min_pos = body
        .iter()
        .enumerate()
        .min_by_key(|(_, s)| s.as_str())
        .map(|(i, _)| i)
        .unwrap_or(0);
    let mut canonical: Vec<String> = body[min_pos..]
        .iter()
        .chain(body[..min_pos].iter())
        .cloned()
        .collect();
    canonical.push(canonical[0].clone()); // close the cycle
    canonical
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(edges: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        edges
            .iter()
            .map(|(k, vs)| (k.to_string(), vs.iter().map(|v| v.to_string()).collect()))
            .collect()
    }

    #[test]
    fn no_cycle() {
        // a → b → c (no cycle)
        let g = graph(&[("a", &["b"]), ("b", &["c"]), ("c", &[])]);
        assert!(detect_cycles(&g).is_empty());
    }

    #[test]
    fn direct_cycle() {
        // a → b → a
        let g = graph(&[("a", &["b"]), ("b", &["a"])]);
        let cycles = detect_cycles(&g);
        assert!(!cycles.is_empty(), "expected a cycle to be detected");
    }

    #[test]
    fn self_loop() {
        // a → a
        let g = graph(&[("a", &["a"])]);
        let cycles = detect_cycles(&g);
        assert!(!cycles.is_empty(), "self-loop should be detected");
    }

    #[test]
    fn longer_cycle() {
        // a → b → c → a
        let g = graph(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"])]);
        let cycles = detect_cycles(&g);
        assert!(!cycles.is_empty(), "3-node cycle should be detected");
        let cycle = &cycles[0];
        // All three nodes should appear in the cycle
        assert!(cycle.contains(&"a".to_string()));
        assert!(cycle.contains(&"b".to_string()));
        assert!(cycle.contains(&"c".to_string()));
    }

    #[test]
    fn disconnected_graph_no_cycle() {
        // a → b; c (isolated)
        let g = graph(&[("a", &["b"]), ("b", &[]), ("c", &[])]);
        assert!(detect_cycles(&g).is_empty());
    }
}
