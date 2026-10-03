//! Cooperative job execution.
//!
//! Noxel's rule is simple: **simulation is deterministic, so any parallel work
//! must be order-independent.** This module provides exactly two shapes of
//! parallelism, and nothing else:
//!
//! * [`JobPool::parallel_chunks`] / [`JobPool::parallel_for`] — *scoped* work
//!   over disjoint slices. The calling thread participates, so a pool with one
//!   thread degrades to a plain loop with identical results. This is how the
//!   rasterizer's tile loop, the BVH build and the NPC crowd update are spread
//!   across cores.
//! * [`JobPool::spawn`] — detached, `'static` background jobs for I/O-shaped
//!   work (loading a texture, generating a chunk). These must never touch
//!   simulation state; they hand results back through a channel or a queue that
//!   the main thread drains at a well-defined point.
//!
//! There is deliberately no work stealing and no `unsafe`: for the tile-shaped
//! workloads here, static chunking is within a few percent of a work-stealing
//! scheduler while being trivial to reason about.
//!
//! ```no_run
//! use noxel_core::jobs::JobPool;
//!
//! let pool = JobPool::new(4);
//! let mut tiles: Vec<u32> = (0..1024).collect();
//! pool.parallel_for(&mut tiles, |i, t| *t = *t + (i as u32));
//! assert_eq!(tiles[10], 20);
//! ```

use std::collections::VecDeque;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

/// A boxed detached job.
type Job = Box<dyn FnOnce() + Send + 'static>;

#[derive(Default)]
struct Shared {
    queue: Mutex<VecDeque<Job>>,
    /// Signalled (with the queue mutex held) when a job is pushed or on shutdown.
    job_available: Condvar,
    /// Number of jobs submitted that have not finished yet.
    pending: Mutex<usize>,
    /// Signalled (with the pending mutex held) when `pending` reaches zero.
    idle: Condvar,
    shutdown: AtomicBool,
}

impl Shared {
    fn take_job(&self) -> Option<Job> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front()
    }

    fn finish_job(&self) {
        let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        *p = p.saturating_sub(1);
        if *p == 0 {
            self.idle.notify_all();
        }
    }
}

/// A fixed-size worker pool.
///
/// Dropping the pool waits for in-flight detached jobs to finish, so a
/// `JobPool` can be dropped mid-frame without losing work.
pub struct JobPool {
    threads: usize,
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl JobPool {
    /// Creates a pool with `threads` workers (clamped to `1..=64`).
    ///
    /// The calling thread is *also* used for scoped work, so `JobPool::new(4)`
    /// spawns three background threads.
    #[must_use]
    pub fn new(threads: usize) -> Self {
        let threads = threads.clamp(1, 64);
        let shared = Arc::new(Shared::default());
        let mut workers = Vec::with_capacity(threads - 1);
        for i in 0..threads.saturating_sub(1) {
            let s = Arc::clone(&shared);
            let handle = std::thread::Builder::new()
                .name(format!("noxel-job-{i}"))
                .spawn(move || worker_loop(&s))
                .expect("failed to spawn Noxel worker thread");
            workers.push(handle);
        }
        Self { threads, shared, workers }
    }

    /// A pool with one worker: all `parallel_*` calls run inline.
    ///
    /// Used by the deterministic test suite and by the `--jobs 1` CLI flag.
    #[must_use]
    pub fn single_threaded() -> Self {
        Self::new(1)
    }

    /// A pool sized to the machine's available parallelism.
    #[must_use]
    pub fn auto() -> Self {
        let n = std::thread::available_parallelism().map_or(1, |n| n.get());
        Self::new(n)
    }

    /// The configured worker count.
    #[inline]
    #[must_use]
    pub fn threads(&self) -> usize {
        self.threads
    }

    /// True when `parallel_*` calls actually run in parallel.
    #[inline]
    #[must_use]
    pub fn is_parallel(&self) -> bool {
        self.threads > 1
    }

    /// Runs `f` over disjoint ranges covering `0..len`.
    ///
    /// The ranges are contiguous and cover the whole input exactly once, so
    /// writing into a shared slice from inside `f` is race-free. `f` may be
    /// called from several threads at once and must therefore be `Sync`.
    ///
    /// With one worker this is a plain loop, which keeps single-threaded and
    /// multi-threaded runs bit-identical.
    pub fn parallel_chunks(&self, len: usize, f: impl Fn(Range<usize>) + Sync) {
        if len == 0 {
            return;
        }
        let threads = self.threads;
        if threads <= 1 || len == 1 {
            f(0..len);
            return;
        }
        let chunk = len.div_ceil(threads);
        let ranges: Vec<Range<usize>> =
            (0..len).step_by(chunk).map(|s| s..(s + chunk).min(len)).collect();
        if ranges.len() == 1 {
            f(ranges[0].clone());
            return;
        }
        std::thread::scope(|scope| {
            let fref = &f;
            for r in ranges.iter().skip(1) {
                let r = r.clone();
                scope.spawn(move || fref(r));
            }
            // The calling thread takes the first chunk rather than idling.
            f(ranges[0].clone());
        });
    }

    /// Runs `f` over every element, in parallel where possible.
    ///
    /// `f` receives the element's index so it can implement gather/scatter.
    pub fn parallel_for<T: Send>(&self, data: &mut [T], f: impl Fn(usize, &mut T) + Sync) {
        let len = data.len();
        if len == 0 {
            return;
        }
        // SAFETY-free disjoint access: split the slice into chunks ourselves and
        // hand each thread exactly one chunk, so two threads can never alias.
        let threads = self.threads;
        if threads <= 1 || len == 1 {
            for (i, item) in data.iter_mut().enumerate() {
                f(i, item);
            }
            return;
        }
        let chunk = len.div_ceil(threads);
        std::thread::scope(|scope| {
            let fref = &f;
            let mut first: Option<&mut [T]> = None;
            let mut rest: Vec<(usize, &mut [T])> = Vec::new();
            for (ci, c) in data.chunks_mut(chunk).enumerate() {
                if ci == 0 {
                    first = Some(c);
                } else {
                    rest.push((ci * chunk, c));
                }
            }
            for (base, c) in rest {
                scope.spawn(move || {
                    for (i, item) in c.iter_mut().enumerate() {
                        fref(base + i, item);
                    }
                });
            }
            if let Some(c) = first {
                for (i, item) in c.iter_mut().enumerate() {
                    f(i, item);
                }
            }
        });
    }

    /// Runs two closures in parallel and returns both results.
    ///
    /// The common "update simulation" / "prepare render data" split.
    pub fn join<A, B, RA, RB>(&self, a: A, b: B) -> (RA, RB)
    where
        A: FnOnce() -> RA + Send,
        B: FnOnce() -> RB + Send,
        RA: Send,
        RB: Send,
    {
        if self.threads <= 1 {
            return (a(), b());
        }
        std::thread::scope(|scope| {
            let ha = scope.spawn(a);
            let rb = b();
            let ra = ha.join().unwrap_or_else(|_| panic!("job panicked"));
            (ra, rb)
        })
    }

    /// Submits a detached background job.
    ///
    /// The job must be `'static` and must not touch simulation state. Typical
    /// uses: reading an asset from disk, decoding a PNG, generating a chunk's
    /// vertex buffer.
    pub fn spawn(&self, job: impl FnOnce() + Send + 'static) {
        {
            let mut p = self.shared.pending.lock().unwrap_or_else(|e| e.into_inner());
            *p += 1;
        }
        if self.threads <= 1 {
            // No worker threads: run inline. Keeps the single-threaded build
            // honest and makes `spawn` + `wait_idle` never deadlock.
            job();
            self.shared.finish_job();
            return;
        }
        let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.push_back(Box::new(job));
        // Notify while holding the queue mutex: the standard pattern that
        // cannot lose a wakeup.
        self.shared.job_available.notify_one();
        drop(q);
    }

    /// Number of submitted jobs that have not finished.
    #[must_use]
    pub fn pending(&self) -> usize {
        *self.shared.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Blocks until every submitted job has finished.
    ///
    /// Detached jobs are drained by the workers themselves, so this does not
    /// need to run any of them.
    pub fn wait_idle(&self) {
        let mut p = self.shared.pending.lock().unwrap_or_else(|e| e.into_inner());
        while *p > 0 {
            p = self.shared.idle.wait(p).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Runs every queued job on the calling thread before returning.
    ///
    /// Used by the headless runner to make frame output independent of worker
    /// timing: after `drain()` the frame is fully resolved.
    pub fn drain(&self) {
        while let Some(job) = self.shared.take_job() {
            job();
            self.shared.finish_job();
        }
    }
}

impl Default for JobPool {
    fn default() -> Self {
        Self::auto()
    }
}

impl core::fmt::Debug for JobPool {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("JobPool").field("threads", &self.threads).field("pending", &self.pending()).finish()
    }
}

fn worker_loop(shared: &Arc<Shared>) {
    loop {
        let job = {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            while q.is_empty() && !shared.shutdown.load(Ordering::Relaxed) {
                let (guard, _timeout) = shared
                    .job_available
                    .wait_timeout(q, std::time::Duration::from_millis(50))
                    .unwrap_or_else(|e| e.into_inner());
                q = guard;
            }
            q.pop_front()
        };
        match job {
            Some(j) => {
                j();
                shared.finish_job();
            }
            None => {
                if shared.shutdown.load(Ordering::Relaxed) {
                    // Drain everything still queued so `Drop` never loses work.
                    while let Some(j) = shared.take_job() {
                        j();
                        shared.finish_job();
                    }
                    return;
                }
            }
        }
    }
}

impl Drop for JobPool {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
        self.shared.job_available.notify_all();
        self.shared.idle.notify_all();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
        // Anything left after the workers exited (a job queued while shutting
        // down) is still executed here so no work is silently lost.
        self.drain();
    }
}

/// A handle for reporting progress from a background job.
///
/// The streaming system uses this to show "generating chunk 12/49" without the
/// generator needing to know where the number is printed.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    inner: Arc<ProgressInner>,
}

#[derive(Debug, Default)]
struct ProgressInner {
    total: AtomicUsize,
    done: AtomicUsize,
    label: Mutex<String>,
}

impl Progress {
    /// Creates a progress counter for `total` units of work.
    #[must_use]
    pub fn new(total: usize, label: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(ProgressInner {
                total: AtomicUsize::new(total),
                done: AtomicUsize::new(0),
                label: Mutex::new(label.into()),
            }),
        }
    }

    /// Marks one unit as done.
    pub fn advance(&self, n: usize) {
        self.inner.done.fetch_add(n, Ordering::Relaxed);
    }

    /// Sets the total (used when a job discovers more work part-way through).
    pub fn set_total(&self, total: usize) {
        self.inner.total.store(total, Ordering::Relaxed);
    }

    /// `(done, total)`.
    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        (self.inner.done.load(Ordering::Relaxed), self.inner.total.load(Ordering::Relaxed))
    }

    /// Completion in `[0, 1]`.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        let (done, total) = self.counts();
        if total == 0 { 1.0 } else { (done as f32 / total as f32).clamp(0.0, 1.0) }
    }

    /// True when all units are done.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        let (done, total) = self.counts();
        done >= total
    }

    /// The human-readable label.
    #[must_use]
    pub fn label(&self) -> String {
        self.inner.label.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Replaces the label.
    pub fn set_label(&self, label: impl Into<String>) {
        *self.inner.label.lock().unwrap_or_else(|e| e.into_inner()) = label.into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_for_covers_every_element_once() {
        let pool = JobPool::new(4);
        let mut data = vec![0u32; 1000];
        pool.parallel_for(&mut data, |i, v| {
            *v = i as u32;
        });
        for (i, v) in data.iter().enumerate() {
            assert_eq!(*v, i as u32);
        }
    }

    #[test]
    fn parallel_for_is_identical_single_threaded() {
        let mut a = vec![0u64; 4096];
        let mut b = vec![0u64; 4096];
        JobPool::new(1).parallel_for(&mut a, |i, v| *v = (i as u64).wrapping_mul(2_654_435_761));
        JobPool::new(8).parallel_for(&mut b, |i, v| *v = (i as u64).wrapping_mul(2_654_435_761));
        assert_eq!(a, b, "parallelism must not change results");
    }

    #[test]
    fn parallel_chunks_partition_exactly() {
        let pool = JobPool::new(7);
        let hits: Mutex<Vec<usize>> = Mutex::new(Vec::new());
        pool.parallel_chunks(100, |r| {
            let mut h = hits.lock().unwrap();
            h.extend(r);
        });
        let mut h = hits.into_inner().unwrap();
        h.sort_unstable();
        assert_eq!(h, (0..100).collect::<Vec<_>>());
    }

    #[test]
    fn parallel_chunks_handles_empty() {
        let pool = JobPool::new(4);
        let called = AtomicUsize::new(0);
        pool.parallel_chunks(0, |_| {
            called.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(called.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn parallel_chunks_handles_tiny_inputs() {
        let pool = JobPool::new(16);
        for len in 1..20usize {
            let hits: Mutex<usize> = Mutex::new(0);
            pool.parallel_chunks(len, |_r| {
                *hits.lock().unwrap() += 1;
            });
            assert!(*hits.lock().unwrap() >= 1, "len {len}");
        }
    }

    #[test]
    fn join_runs_both_closures() {
        let pool = JobPool::new(4);
        let (a, b) = pool.join(|| 1 + 1, || 2 + 2);
        assert_eq!((a, b), (2, 4));
    }

    #[test]
    fn detached_jobs_are_drained() {
        let pool = JobPool::new(4);
        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..100 {
            let c = Arc::clone(&counter);
            pool.spawn(move || {
                c.fetch_add(1, Ordering::Relaxed);
            });
        }
        pool.wait_idle();
        assert_eq!(counter.load(Ordering::Relaxed), 100);
        assert_eq!(pool.pending(), 0);
    }

    #[test]
    fn spawn_runs_inline_when_single_threaded() {
        let pool = JobPool::single_threaded();
        let flag = Arc::new(AtomicBool::new(false));
        let f = Arc::clone(&flag);
        pool.spawn(move || f.store(true, Ordering::Relaxed));
        assert!(flag.load(Ordering::Relaxed));
    }

    #[test]
    fn drain_executes_queued_work() {
        let pool = JobPool::new(4);
        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..10 {
            let c = Arc::clone(&counter);
            pool.spawn(move || {
                c.fetch_add(1, Ordering::Relaxed);
            });
        }
        pool.drain();
        pool.wait_idle();
        assert_eq!(counter.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn auto_pool_has_at_least_one_thread() {
        assert!(JobPool::auto().threads() >= 1);
    }

    #[test]
    fn progress_tracks_fraction() {
        let p = Progress::new(4, "chunks");
        assert_eq!(p.fraction(), 0.0);
        p.advance(2);
        assert!((p.fraction() - 0.5).abs() < 1e-6);
        p.advance(2);
        assert!(p.is_complete());
        assert_eq!(p.label(), "chunks");
    }

    #[test]
    fn progress_with_zero_total_is_complete() {
        let p = Progress::new(0, "nothing");
        assert!(p.is_complete());
        assert_eq!(p.fraction(), 1.0);
    }

    #[test]
    fn dropping_pool_with_pending_work_completes_it() {
        let counter = Arc::new(AtomicUsize::new(0));
        {
            let pool = JobPool::new(4);
            for _ in 0..50 {
                let c = Arc::clone(&counter);
                pool.spawn(move || {
                    c.fetch_add(1, Ordering::Relaxed);
                });
            }
            assert!(pool.pending() <= 50);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 50, "Drop must not lose jobs");
    }
}
