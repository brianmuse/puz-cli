//! Running engine work on several threads. In WebAssembly there are no threads, so the same calls
//! run one item at a time on the calling thread (a Web Worker, so the page stays responsive).

use anyhow::Result;

/// Engine work (word lists are ~100MB each) runs on at most this many threads.
pub const MAX_THREADS: usize = 4;

/// How many threads to use, at most `max`. Always 1 in WebAssembly.
pub fn threads(max: usize) -> usize {
    if cfg!(target_arch = "wasm32") {
        return 1;
    }
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, max.max(1))
}

/// How many items run at once with `parallel`. Callers use it to split a time budget.
pub fn max_threads() -> usize {
    if cfg!(target_arch = "wasm32") { 1 } else { MAX_THREADS }
}

/// Run `f` on each item, at most MAX_THREADS at a time, keeping the order.
pub fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    parallel_upto(items, MAX_THREADS, f)
}

/// Run `f` on each item, at most `max` at a time (and no more than the machine has), keeping the
/// order.
pub fn parallel_upto<T: Sync, R: Send>(items: &[T], max: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let n = threads(max);
    if n == 1 {
        return items.iter().map(&f).collect();
    }
    let mut out = vec![];
    for chunk in items.chunks(n) {
        let r: Vec<R> = std::thread::scope(|s| {
            let hs: Vec<_> = chunk.iter().map(|t| s.spawn(|| f(t))).collect();
            hs.into_iter().map(|h| h.join().expect("engine thread panicked")).collect()
        });
        out.extend(r);
    }
    out
}

/// Run `n` copies of a worker loop at once and wait for them all. With one thread the worker runs
/// on the calling thread.
pub fn workers(n: usize, f: impl Fn() -> Result<()> + Sync) -> Result<()> {
    if n <= 1 {
        return f();
    }
    std::thread::scope(|s| -> Result<()> {
        let hs: Vec<_> = (0..n).map(|_| s.spawn(&f)).collect();
        for h in hs {
            h.join().expect("engine thread panicked")?;
        }
        Ok(())
    })
}
