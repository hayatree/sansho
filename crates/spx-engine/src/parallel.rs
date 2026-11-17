//! Minimal deterministic parallel map over independent jobs (parallelism level L0).
//!
//! Jobs are claimed dynamically by worker threads, but every result is stored at its job
//! index, so the output never depends on scheduling or on the number of threads.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Number of worker threads to use when the caller does not specify one.
pub fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

/// Apply `f` to `0..n` using up to `threads` threads; results are returned in index order.
pub fn map<T: Send>(n: usize, threads: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let threads = threads.clamp(1, n.max(1));
    if threads == 1 {
        return (0..n).map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<T>>> = (0..n).map(|_| Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let r = f(i);
                    if let Ok(mut slot) = slots[i].lock() {
                        *slot = Some(r);
                    }
                }
            });
        }
    });
    slots
        .into_iter()
        .filter_map(|m| m.into_inner().ok().flatten())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_are_in_index_order() {
        for threads in [1, 2, 7] {
            let v = map(100, threads, |i| i * i);
            assert_eq!(v, (0..100).map(|i| i * i).collect::<Vec<_>>());
        }
    }
}
