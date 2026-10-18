//! Small, bounded batches of local frame work. No task or filesystem writes.
//!
//! The owning coordinator keeps the execution ledger and publishes results only
//! after this function returns. `work` must honor its batch-local cancellation
//! token and returned time allowance; Rust cannot forcibly stop a scoped thread.

use crate::domain::{AgentError, CancelToken, Result};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(10);

pub(crate) struct BatchResults<T> {
    /// Every input index has one result, regardless of completion order.
    pub results: Vec<Result<T>>,
    /// Observed work callbacks in flight, not the configured ceiling.
    pub peak_workers: u32,
    /// Sum of measured callback wall times; diagnostic, not task elapsed time.
    pub worker_ms: u64,
}

#[derive(Default)]
struct Metrics {
    active: AtomicU32,
    peak: AtomicU32,
    worker_ms: AtomicU64,
}

struct Active<'a> {
    metrics: &'a Metrics,
    started: Instant,
}

impl Metrics {
    fn enter(&self) -> Active<'_> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        Active {
            metrics: self,
            started: Instant::now(),
        }
    }
}

impl Drop for Active<'_> {
    fn drop(&mut self) {
        let ms = self.started.elapsed().as_nanos().div_ceil(1_000_000);
        let ms = ms.min(u128::from(u64::MAX)) as u64;
        let _ =
            self.metrics
                .worker_ms
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                    Some(current.saturating_add(ms))
                });
        self.metrics.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Run at most one wave of `ceiling` callbacks. Runtime failures are returned at
/// their input indices, so the owner can inspect successful work without losing
/// diagnostics. Only invalid batch arguments produce an outer error.
pub(crate) fn run<T: Send, F>(
    count: usize,
    ceiling: usize,
    cancel: &CancelToken,
    deadline: Instant,
    work: F,
) -> Result<BatchResults<T>>
where
    F: Fn(usize, &CancelToken, Duration) -> Result<T> + Sync,
{
    if !(1..=4).contains(&ceiling) || count > ceiling {
        return Err(AgentError::Invalid(
            "Frame batch size must not exceed its 1–4 worker ceiling".into(),
        ));
    }
    if count == 0 {
        return Ok(BatchResults {
            results: Vec::new(),
            peak_workers: 0,
            worker_ms: 0,
        });
    }
    let stopped = if cancel.is_cancelled() {
        Some(false)
    } else if Instant::now() >= deadline {
        Some(true)
    } else {
        None
    };
    if let Some(timed_out) = stopped {
        return Ok(BatchResults {
            results: (0..count)
                .map(|_| {
                    Err(if timed_out {
                        AgentError::TimedOut
                    } else {
                        AgentError::Cancelled
                    })
                })
                .collect(),
            peak_workers: 0,
            worker_ms: 0,
        });
    }

    let batch_cancel = CancelToken::default();
    let metrics = Metrics::default();
    let (sender, receiver) = mpsc::channel();
    let mut ordered: Vec<Option<Result<T>>> = (0..count).map(|_| None).collect();

    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(count);
        for index in 0..count {
            let sender = sender.clone();
            let batch_cancel = &batch_cancel;
            let work = &work;
            let metrics = &metrics;
            let spawned = thread::Builder::new()
                .name(format!("video-agent-frame-{index}"))
                .spawn_scoped(scope, move || {
                    let result = if cancel.is_cancelled() {
                        Err(AgentError::Cancelled)
                    } else if Instant::now() >= deadline {
                        Err(AgentError::TimedOut)
                    } else if batch_cancel.is_cancelled() {
                        Err(AgentError::Cancelled)
                    } else {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        let result = if remaining.is_zero() {
                            Err(AgentError::TimedOut)
                        } else {
                            let active = metrics.enter();
                            let result = catch_unwind(AssertUnwindSafe(|| {
                                work(index, batch_cancel, remaining)
                            }))
                            .unwrap_or_else(|_| {
                                Err(AgentError::Invalid("Frame worker panicked".into()))
                            });
                            drop(active);
                            result
                        };
                        // A deadline-triggered peer cancellation must not be
                        // mislabeled as an explicit human cancellation. Keep
                        // other concrete worker errors intact.
                        if Instant::now() >= deadline
                            && !cancel.is_cancelled()
                            && (result.is_ok() || matches!(result, Err(AgentError::Cancelled)))
                        {
                            Err(AgentError::TimedOut)
                        } else {
                            result
                        }
                    };
                    if result.is_err() {
                        // Do not wait for the coordinator's next poll to stop
                        // peers after a local error or panic.
                        batch_cancel.cancel();
                    }
                    let _ = sender.send((index, result));
                });
            match spawned {
                Ok(handle) => handles.push((index, handle)),
                Err(error) => {
                    batch_cancel.cancel();
                    ordered[index] = Some(Err(AgentError::Io(error)));
                    for result in ordered.iter_mut().skip(index + 1) {
                        *result = Some(Err(AgentError::Cancelled));
                    }
                    break;
                }
            }
        }
        drop(sender);

        while ordered.iter().any(Option::is_none) {
            if cancel.is_cancelled() || Instant::now() >= deadline {
                batch_cancel.cancel();
            }
            // After expiration, still poll while draining cooperative workers.
            let remaining = deadline.saturating_duration_since(Instant::now());
            let wait = if remaining.is_zero() {
                POLL
            } else {
                remaining.min(POLL)
            };
            match receiver.recv_timeout(wait) {
                Ok((index, result)) => {
                    if result.is_err() {
                        batch_cancel.cancel();
                    }
                    ordered[index] = Some(result);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    batch_cancel.cancel();
                    break;
                }
            }
        }
        // All scoped handles are explicitly joined before any result escapes;
        // a callback cannot remain in the background after cancellation.
        for (index, handle) in handles {
            if handle.join().is_err() {
                batch_cancel.cancel();
                ordered[index] = Some(Err(AgentError::Invalid("Frame worker panicked".into())));
            }
        }
    });

    Ok(BatchResults {
        results: ordered
            .into_iter()
            .map(|result| {
                result.unwrap_or_else(|| {
                    Err(AgentError::Invalid(
                        "Frame worker ended without returning a result".into(),
                    ))
                })
            })
            .collect(),
        peak_workers: metrics.peak.load(Ordering::SeqCst),
        worker_ms: metrics.worker_ms.load(Ordering::SeqCst),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Barrier, Mutex};

    struct InFlight<'a>(&'a AtomicUsize);
    impl Drop for InFlight<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn one_two_and_four_workers_really_overlap_and_remain_bounded() {
        for width in [1, 2, 4] {
            let barrier = Barrier::new(width);
            let active = AtomicUsize::new(0);
            let peak = AtomicUsize::new(0);
            let parent = CancelToken::default();
            let batch = run(
                width,
                width,
                &parent,
                Instant::now() + Duration::from_secs(3),
                |index, token, remaining| {
                    assert!(!token.is_cancelled());
                    assert!(!remaining.is_zero());
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    let _guard = InFlight(&active);
                    peak.fetch_max(now, Ordering::SeqCst);
                    assert!(now <= width);
                    barrier.wait();
                    thread::sleep(Duration::from_millis(3));
                    Ok(index)
                },
            )
            .unwrap();
            assert_eq!(batch.peak_workers, width as u32);
            assert_eq!(peak.load(Ordering::SeqCst), width);
            assert_eq!(active.load(Ordering::SeqCst), 0);
            assert!(batch.worker_ms >= width as u64);
            assert_eq!(
                batch
                    .results
                    .into_iter()
                    .map(Result::unwrap)
                    .collect::<Vec<_>>(),
                (0..width).collect::<Vec<_>>()
            );
            assert!(!parent.is_cancelled());
        }
    }

    #[test]
    fn out_of_order_completions_return_in_input_order() {
        let barrier = Barrier::new(4);
        let turn = AtomicUsize::new(3);
        let completions = Mutex::new(Vec::new());
        let batch = run(
            4,
            4,
            &CancelToken::default(),
            Instant::now() + Duration::from_secs(3),
            |index, token, remaining| {
                let deadline = Instant::now() + remaining;
                barrier.wait();
                while turn.load(Ordering::SeqCst) != index {
                    token.check()?;
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(1));
                }
                completions.lock().unwrap().push(index);
                turn.store(index.saturating_sub(1), Ordering::SeqCst);
                Ok(index)
            },
        )
        .unwrap();
        assert_eq!(*completions.lock().unwrap(), [3, 2, 1, 0]);
        assert_eq!(
            batch
                .results
                .into_iter()
                .map(Result::unwrap)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn parent_cancellation_drains_all_workers() {
        let parent = CancelToken::default();
        let entered = AtomicUsize::new(0);
        let active = AtomicUsize::new(0);
        let batch = thread::scope(|scope| {
            scope.spawn(|| {
                let deadline = Instant::now() + Duration::from_secs(3);
                while entered.load(Ordering::SeqCst) != 4 {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(1));
                }
                parent.cancel();
            });
            run(
                4,
                4,
                &parent,
                Instant::now() + Duration::from_secs(4),
                |_, token, remaining| {
                    active.fetch_add(1, Ordering::SeqCst);
                    let _guard = InFlight(&active);
                    entered.fetch_add(1, Ordering::SeqCst);
                    let deadline = Instant::now() + remaining;
                    while !token.is_cancelled() {
                        assert!(Instant::now() < deadline);
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err::<(), _>(AgentError::Cancelled)
                },
            )
            .unwrap()
        });
        assert_eq!(entered.load(Ordering::SeqCst), 4);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert!(batch
            .results
            .iter()
            .all(|r| matches!(r, Err(AgentError::Cancelled))));
    }

    #[test]
    fn first_error_cancels_peers_and_does_not_cancel_parent() {
        let parent = CancelToken::default();
        let barrier = Barrier::new(4);
        let active = AtomicUsize::new(0);
        let batch = run(
            4,
            4,
            &parent,
            Instant::now() + Duration::from_secs(3),
            |index, token, remaining| {
                active.fetch_add(1, Ordering::SeqCst);
                let _guard = InFlight(&active);
                let deadline = Instant::now() + remaining;
                barrier.wait();
                if index == 1 {
                    return Err::<(), _>(AgentError::Invalid("fixture failure".into()));
                }
                while !token.is_cancelled() {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(1));
                }
                Err(AgentError::Cancelled)
            },
        )
        .unwrap();
        assert!(matches!(&batch.results[1], Err(AgentError::Invalid(s)) if s == "fixture failure"));
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert!(!parent.is_cancelled());
    }

    #[test]
    fn panicking_worker_is_an_error_and_all_peers_are_joined() {
        let parent = CancelToken::default();
        let barrier = Barrier::new(4);
        let active = AtomicUsize::new(0);
        let batch = run(
            4,
            4,
            &parent,
            Instant::now() + Duration::from_secs(3),
            |index, token, remaining| {
                active.fetch_add(1, Ordering::SeqCst);
                let _guard = InFlight(&active);
                let deadline = Instant::now() + remaining;
                barrier.wait();
                assert_ne!(index, 2, "fixture worker panic");
                while !token.is_cancelled() {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(1));
                }
                Err::<(), _>(AgentError::Cancelled)
            },
        )
        .unwrap();
        assert!(
            matches!(&batch.results[2], Err(AgentError::Invalid(s)) if s == "Frame worker panicked")
        );
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert!(!parent.is_cancelled());
    }

    #[test]
    fn absolute_deadline_cancels_running_workers_without_mislabeling() {
        let parent = CancelToken::default();
        let active = AtomicUsize::new(0);
        let batch = run(
            2,
            2,
            &parent,
            Instant::now() + Duration::from_millis(50),
            |_, token, _| {
                active.fetch_add(1, Ordering::SeqCst);
                let _guard = InFlight(&active);
                while !token.is_cancelled() {
                    thread::sleep(Duration::from_millis(1));
                }
                Err::<(), _>(AgentError::Cancelled)
            },
        )
        .unwrap();
        assert!(batch
            .results
            .iter()
            .all(|r| matches!(r, Err(AgentError::TimedOut))));
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert!(!parent.is_cancelled());
    }

    #[test]
    fn expired_or_precancelled_batches_do_not_enter_work() {
        for cancelled in [false, true] {
            let parent = CancelToken::default();
            if cancelled {
                parent.cancel();
            }
            let calls = AtomicUsize::new(0);
            let deadline = if cancelled {
                Instant::now() + Duration::from_secs(3)
            } else {
                Instant::now() - Duration::from_millis(1)
            };
            let batch = run(4, 4, &parent, deadline, |_, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert_eq!(batch.peak_workers, 0);
            assert_eq!(batch.worker_ms, 0);
            assert!(batch.results.iter().all(|r| if cancelled {
                matches!(r, Err(AgentError::Cancelled))
            } else {
                matches!(r, Err(AgentError::TimedOut))
            }));
        }
    }

    #[test]
    fn empty_batches_and_invalid_bounds_do_not_enter_work() {
        let parent = CancelToken::default();
        let calls = AtomicUsize::new(0);
        let work = |_, _: &CancelToken, _: Duration| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let empty = run(0, 4, &parent, Instant::now(), work).unwrap();
        assert!(empty.results.is_empty());
        assert_eq!(empty.peak_workers, 0);
        assert_eq!(empty.worker_ms, 0);
        for (count, ceiling) in [(1, 0), (0, 5), (2, 1), (5, 4)] {
            assert!(run(count, ceiling, &parent, Instant::now(), work).is_err());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(!parent.is_cancelled());
    }
}
