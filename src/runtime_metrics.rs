//! Periodically publishes tokio's own per-worker runtime metrics (busy time, local queue
//! depth, mean poll time) so scheduler-level bottlenecks - a worker thread monopolized by
//! a long-running task, or a backlog of runnable tasks the scheduler can't drain fast
//! enough - are directly observable instead of only inferable from aggregate CPU numbers.
//!
//! Requires the binary to be built with `--cfg tokio_unstable` (tokio gates
//! [`tokio::runtime::RuntimeMetrics`]'s per-worker accessors behind it, since their exact
//! shape isn't yet stabilized). [`spawn_collector`] is a no-op when that flag isn't set, so
//! call sites don't need their own `#[cfg(tokio_unstable)]` guard.

use std::time::Duration;

/// Spawns a task that samples tokio's runtime metrics every `interval` and republishes them
/// as gauges/counters through the same recorder every other metric in this crate uses. A
/// no-op unless built with `--cfg tokio_unstable`.
pub fn spawn_collector(interval: Duration) {
    #[cfg(tokio_unstable)]
    {
        tokio::spawn(collect(interval));
    }
    #[cfg(not(tokio_unstable))]
    {
        let _ = interval;
    }
}

#[cfg(tokio_unstable)]
async fn collect(interval: Duration) {
    let handle = tokio::runtime::Handle::current();
    let metrics = handle.metrics();
    let num_workers = metrics.num_workers();

    // One gauge/counter handle per worker, obtained once rather than re-registered every
    // tick (same reasoning as the per-call metrics elsewhere in this crate - avoids a
    // registry lookup on every sample).
    let worker_labels: Vec<String> = (0..num_workers).map(|i| i.to_string()).collect();
    let busy_nanos: Vec<_> = worker_labels
        .iter()
        .map(|w| crate::metrics::tokio_worker_busy_nanos_total(w.clone()))
        .collect();
    let local_queue_depth: Vec<_> = worker_labels
        .iter()
        .map(|w| crate::metrics::tokio_worker_local_queue_depth(w.clone()))
        .collect();
    let mean_poll_time: Vec<_> = worker_labels
        .iter()
        .map(|w| crate::metrics::tokio_worker_mean_poll_time_seconds(w.clone()))
        .collect();
    let global_queue_depth = crate::metrics::tokio_global_queue_depth();
    let num_alive_tasks = crate::metrics::tokio_num_alive_tasks();

    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;

        global_queue_depth.set(metrics.global_queue_depth() as f64);
        num_alive_tasks.set(metrics.num_alive_tasks() as f64);
        for i in 0..num_workers {
            busy_nanos[i].absolute(metrics.worker_total_busy_duration(i).as_nanos() as u64);
            local_queue_depth[i].set(metrics.worker_local_queue_depth(i) as f64);
            mean_poll_time[i].set(metrics.worker_mean_poll_time(i).as_secs_f64());
        }
    }
}
