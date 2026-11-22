//! Ordering of a square circuit block on the undirected pattern of A + Aᵀ.
//! Provenance: original implementation from Amestoy, Davis & Duff (1996)
//! and George & Liu (1981, 1989), using a quotient graph of elimination elements.

use crate::CscPattern;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Immutable block input and elimination order shared across numeric jobs.
#[derive(Debug)]
pub struct BatchOrdering {
    pub(crate) input: Vec<Vec<(usize, usize)>>,
    pub(crate) order: Vec<usize>,
}

impl BatchOrdering {
    pub fn dimension(&self) -> usize {
        self.order.len()
    }

    pub fn new(pattern: &CscPattern) -> Result<Arc<Self>, crate::SolverError> {
        let input = pattern
            .col_ptr()
            .windows(2)
            .map(|bounds| {
                (bounds[0]..bounds[1])
                    .map(|k| (pattern.row_idx()[k], k))
                    .collect()
            })
            .collect();
        Ok(Self::block(input))
    }

    pub(crate) fn block(input: Vec<Vec<(usize, usize)>>) -> Arc<Self> {
        let order = minimum_degree(&input);
        Arc::new(Self { input, order })
    }
}

fn minimum_degree(input: &[Vec<(usize, usize)>]) -> Vec<usize> {
    let n = input.len();
    if n < 4 {
        return (0..n).collect();
    }
    let mut edges = vec![Vec::new(); n];
    for (j, col) in input.iter().enumerate() {
        for &(i, _) in col {
            if i != j {
                edges[i].push(j);
                edges[j].push(i);
            }
        }
    }
    for row in &mut edges {
        row.sort_unstable();
        row.dedup();
    }
    // Dense rails are eliminated last. Their incident edges need not
    // participate in the quotient graph of the remaining variables.
    let limit = 16.max((10. * (n as f64).sqrt()) as usize);
    let dense: Vec<_> = (0..n).filter(|&v| edges[v].len() > limit).collect();
    let mut live = vec![true; n];
    for &v in &dense {
        live[v] = false;
    }
    for row in &mut edges {
        row.retain(|&w| live[w]);
    }
    let tier = separator_levels(&edges, &live);
    let mut degree: Vec<_> = edges.iter().map(Vec::len).collect();
    let mut queue: BTreeSet<_> = (0..n)
        .filter(|&v| live[v])
        .map(|v| (tier[v], degree[v], v))
        .collect();
    let mut elements = vec![Vec::<usize>::new(); n];
    let mut incident = vec![Vec::<usize>::new(); n];
    let mut boundary = Vec::new();
    let mut stamp = vec![n; n];
    let mut outside = vec![0; n];
    let mut counted = vec![n; n];
    let mut order = Vec::with_capacity(n);
    while let Some((_, _, pivot)) = queue.pop_first() {
        live[pivot] = false;
        order.push(pivot);
        boundary.clear();
        let mut include = |w| {
            if live[w] && stamp[w] != pivot {
                stamp[w] = pivot;
                boundary.push(w);
            }
        };
        for w in edges[pivot].drain(..) {
            include(w);
        }
        // Every incident element is absorbed by the new boundary clique.
        for e in incident[pivot].drain(..) {
            for w in elements[e].drain(..) {
                include(w);
            }
        }
        // Count each surviving element's external variables once per pivot.
        // Elements wholly contained in the boundary can also be absorbed.
        for &w in &boundary {
            for &e in &incident[w] {
                if counted[e] != pivot {
                    counted[e] = pivot;
                    outside[e] = elements[e].iter().filter(|&&i| stamp[i] != pivot).count();
                    if outside[e] == 0 {
                        elements[e].clear();
                    }
                }
            }
        }
        for &w in &boundary {
            queue.remove(&(tier[w], degree[w], w));
            edges[w].retain(|&i| live[i] && stamp[i] != pivot);
            incident[w].retain(|&e| !elements[e].is_empty());
            if tier[w] <= tier[pivot] && edges[w].is_empty() && incident[w].is_empty() {
                // All live neighbors are already in the new clique. Eliminate
                // these indistinguishable variables together with the pivot.
                live[w] = false;
                order.push(w);
            }
        }
        boundary.retain(|&w| live[w]);
        for &w in &boundary {
            // Overlaps between external elements give an upper bound on degree.
            degree[w] = boundary.len() - 1
                + edges[w].len()
                + incident[w].iter().map(|&e| outside[e]).sum::<usize>();
            incident[w].push(pivot);
            queue.insert((tier[w], degree[w], w));
        }
        elements[pivot].clone_from(&boundary);
    }
    order.extend(dense);
    order
}

// Recursively delay narrow level separators on large mesh-like blocks
// (George & Liu, 1981). Within each region the quotient degree chooses pivots.
fn separator_levels(edges: &[Vec<usize>], live: &[bool]) -> Vec<usize> {
    let mut levels = vec![0; edges.len()];
    if edges.len() >= 16_384 {
        let vertices = (0..edges.len()).filter(|&i| live[i]).collect::<Vec<_>>();
        split(
            edges,
            vertices,
            &mut vec![usize::MAX; edges.len()],
            &mut Vec::new(),
            &mut levels,
        );
    }
    levels
}

fn split(
    edges: &[Vec<usize>],
    vertices: Vec<usize>,
    distance: &mut [usize],
    queue: &mut Vec<usize>,
    levels: &mut [usize],
) -> usize {
    if vertices.len() < 1024 {
        return 0;
    }
    let n = edges.len();
    let mut root = vertices[0];
    // A breadth-first sweep supplies a peripheral starting vertex for the
    // second sweep. Inactive vertices retain the out-of-region sentinel.
    for _ in 0..2 {
        for &v in &vertices {
            distance[v] = n;
        }
        queue.clear();
        queue.push(root);
        distance[root] = 0;
        let mut pos = 0;
        while pos < queue.len() {
            let v = queue[pos];
            for &w in &edges[v] {
                if distance[w] == n {
                    distance[w] = distance[v] + 1;
                    queue.push(w);
                }
            }
            pos += 1;
        }
        root = queue[pos - 1];
    }
    let mut offset = 0;
    let mut narrowest = usize::MAX;
    let mut cut = None;
    if queue.len() == vertices.len() {
        for layer in queue.chunk_by(|a, b| distance[*a] == distance[*b]) {
            let end = offset + layer.len();
            // Keep at least a quarter of the region on either side. Use a
            // separator only when its width is small relative to both sides.
            let smaller = offset.min(vertices.len() - end);
            if smaller >= vertices.len() / 4
                && layer.len() <= smaller / 4
                && layer.len() < narrowest
            {
                narrowest = layer.len();
                cut = Some(distance[layer[0]]);
            }
            offset = end;
        }
    }
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut separator = Vec::new();
    for v in vertices {
        let d = std::mem::replace(&mut distance[v], usize::MAX);
        if let Some(cut) = cut {
            match d.cmp(&cut) {
                std::cmp::Ordering::Less => left.push(v),
                std::cmp::Ordering::Greater => right.push(v),
                std::cmp::Ordering::Equal => separator.push(v),
            }
        }
    }
    if separator.is_empty() {
        return 0;
    }
    let height = 1 + split(edges, left, distance, queue, levels)
        .max(split(edges, right, distance, queue, levels));
    for v in separator {
        levels[v] = height;
    }
    height
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_rail_is_last_and_input_visit_order_does_not_change_ties() {
        let n = 401;
        let mut input: Vec<Vec<_>> = (0..n)
            .map(|j| vec![(j, 0), (0, 0), ((j + 1) % n, 0)])
            .collect();
        let order = minimum_degree(&input);
        assert_eq!(order.last(), Some(&0));
        for rows in &mut input {
            rows.reverse();
        }
        assert_eq!(order, minimum_degree(&input));
        let mut all = order;
        all.sort_unstable();
        assert!(all.into_iter().eq(0..n));
    }
}
