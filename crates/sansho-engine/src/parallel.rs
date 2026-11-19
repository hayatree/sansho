//! Thread-pool handling for all parallelism levels.
//!
//! Every parallel loop in the engine writes results to fixed positions and sums in a fixed
//! order, so the output never depends on scheduling or on the number of threads.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

/// Number of worker threads to use when the caller does not specify one.
pub fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

/// Run `f` inside a rayon pool with `threads` workers. If the pool cannot be created, `f` runs
/// in the caller's current pool instead (results are the same either way).
pub(crate) fn with_threads<R: Send>(threads: usize, f: impl FnOnce() -> R + Send) -> R {
    match rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .build()
    {
        Ok(pool) => pool.install(f),
        Err(_) => f(),
    }
}
