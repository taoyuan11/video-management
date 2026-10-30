//! One application-owned executor for hashing and cover extraction.
use crate::model::AppSettings;
use std::{
    collections::VecDeque,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

#[derive(Clone, Copy)]
pub enum Lane {
    Scan = 0,
    Cover = 1,
}

struct Job {
    cancel: Arc<AtomicBool>,
    run: Box<dyn FnOnce() + Send>,
}
struct State {
    queues: [VecDeque<Job>; 2],
    running: Vec<Arc<AtomicBool>>,
    limit: usize,
    next: usize,
    stopped: bool,
}
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Clone)]
pub struct Executor(Arc<Shared>);

impl Executor {
    pub fn submit(
        &self,
        lane: Lane,
        cancel: Arc<AtomicBool>,
        run: impl FnOnce() + Send + 'static,
    ) -> bool {
        let mut state = self.0.state.lock().unwrap();
        if state.stopped {
            return false;
        }
        state.queues[lane as usize].push_back(Job {
            cancel,
            run: Box::new(run),
        });
        self.0.wake.notify_all();
        true
    }

    /// Drop cancelled queued work without waiting for a busy worker to become free.
    pub fn discard_cancelled(&self) {
        let mut state = self.0.state.lock().unwrap();
        for queue in &mut state.queues {
            queue.retain(|job| !job.cancel.load(Ordering::Relaxed));
        }
    }
}

pub struct Pool {
    executor: Executor,
    workers: Vec<JoinHandle<()>>,
}
impl Pool {
    pub fn new(limit: usize) -> io::Result<Self> {
        let mut pool = Self {
            executor: Executor(Arc::new(Shared {
                state: Mutex::new(State {
                    queues: Default::default(),
                    running: Vec::new(),
                    limit: 1,
                    next: 0,
                    stopped: false,
                }),
                wake: Condvar::new(),
            })),
            workers: Vec::new(),
        };
        pool.prepare(limit)?;
        pool.set_limit(limit);
        Ok(pool)
    }
    pub fn executor(&self) -> Executor {
        self.executor.clone()
    }

    /// Allocate before persisting settings; failure leaves the previous limit in effect.
    pub fn prepare(&mut self, limit: usize) -> io::Result<()> {
        let max = AppSettings::max_concurrency();
        if !(1..=max).contains(&limit) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("并发线程数必须为 1–{max}"),
            ));
        }
        while self.workers.len() < limit {
            let index = self.workers.len();
            let shared = self.executor.0.clone();
            self.workers.push(
                thread::Builder::new()
                    .name(format!("video-processing-{index}"))
                    .spawn(move || worker(shared, index))?,
            );
        }
        Ok(())
    }
    pub fn set_limit(&self, limit: usize) {
        assert!((1..=self.workers.len()).contains(&limit));
        self.executor.0.state.lock().unwrap().limit = limit;
        self.executor.0.wake.notify_all();
    }
    pub fn shutdown(&mut self) {
        {
            let mut state = self.executor.0.state.lock().unwrap();
            state.stopped = true;
            for cancel in &state.running {
                cancel.store(true, Ordering::Relaxed);
            }
            for queue in &mut state.queues {
                queue.clear();
            }
            self.executor.0.wake.notify_all();
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn worker(shared: Arc<Shared>, index: usize) {
    loop {
        let job = {
            let mut state = shared.state.lock().unwrap();
            loop {
                if state.stopped {
                    return;
                }
                if index < state.limit && state.running.len() < state.limit {
                    let first = state.next;
                    if let Some((lane, job)) = [first, 1 - first]
                        .into_iter()
                        .find_map(|lane| state.queues[lane].pop_front().map(|job| (lane, job)))
                    {
                        if job.cancel.load(Ordering::Relaxed) {
                            continue;
                        }
                        state.next = 1 - lane;
                        state.running.push(job.cancel.clone());
                        break job;
                    }
                }
                state = shared.wake.wait(state).unwrap();
            }
        };
        let _ = catch_unwind(AssertUnwindSafe(job.run));
        let mut state = shared.state.lock().unwrap();
        if let Some(index) = state
            .running
            .iter()
            .position(|flag| Arc::ptr_eq(flag, &job.cancel))
        {
            state.running.swap_remove(index);
        }
        shared.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
    use std::time::Duration;

    fn gated(executor: &Executor, lane: Lane, id: usize, started: Sender<usize>) -> Sender<()> {
        let (release, wait) = bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        executor.submit(lane, cancel.clone(), move || {
            started.send(id).unwrap();
            while !cancel.load(Ordering::Relaxed)
                && wait.recv_timeout(Duration::from_millis(20)).is_err()
            {}
        });
        release
    }
    fn next(rx: &Receiver<usize>) -> usize {
        rx.recv_timeout(Duration::from_secs(3)).unwrap()
    }

    #[test]
    fn invalid_cpu_limit_does_not_change_pool_capacity() {
        let mut pool = Pool::new(1).unwrap();
        for limit in [0, AppSettings::max_concurrency() + 1] {
            let error = pool.prepare(limit).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(
                error
                    .to_string()
                    .contains(&format!("1–{}", AppSettings::max_concurrency()))
            );
            assert_eq!(pool.workers.len(), 1);
            assert_eq!(pool.executor.0.state.lock().unwrap().limit, 1);
        }
        pool.shutdown();
    }

    #[test]
    fn both_lanes_share_limit_and_resize_without_interrupting_active_work() {
        if AppSettings::max_concurrency() < 2 {
            return; // Resizing requires at least two available logical processors.
        }
        let mut pool = Pool::new(1).unwrap();
        let executor = pool.executor();
        let (started, rx) = unbounded();
        let first = gated(&executor, Lane::Scan, 1, started.clone());
        assert_eq!(next(&rx), 1);
        let second = gated(&executor, Lane::Cover, 2, started.clone());
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
        pool.prepare(2).unwrap();
        pool.set_limit(2);
        assert_eq!(next(&rx), 2);
        pool.set_limit(1);
        let third = gated(&executor, Lane::Scan, 3, started);
        first.send(()).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
        second.send(()).unwrap();
        assert_eq!(next(&rx), 3);
        third.send(()).unwrap();
        pool.shutdown();
    }

    #[test]
    fn scheduling_alternates_lanes_and_a_panic_does_not_kill_the_worker() {
        let mut pool = Pool::new(1).unwrap();
        let executor = pool.executor();
        let (tx, rx) = unbounded();
        let release = gated(&executor, Lane::Scan, 99, tx.clone());
        assert_eq!(next(&rx), 99);
        for (lane, id) in [
            (Lane::Scan, 1),
            (Lane::Scan, 3),
            (Lane::Cover, 0),
            (Lane::Cover, 2),
        ] {
            let tx = tx.clone();
            executor.submit(lane, Arc::new(AtomicBool::new(false)), move || {
                tx.send(id).unwrap();
            });
        }
        release.send(()).unwrap();
        assert_eq!(
            (0..4).map(|_| next(&rx)).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        executor.submit(Lane::Scan, Arc::new(AtomicBool::new(false)), || {
            panic!("test task failure")
        });
        executor.submit(Lane::Scan, Arc::new(AtomicBool::new(false)), move || {
            tx.send(4).unwrap();
        });
        assert_eq!(next(&rx), 4);
        pool.shutdown();
    }

    #[test]
    fn cancellation_discards_queued_work_and_shutdown_joins_active_tasks() {
        let mut pool = Pool::new(1).unwrap();
        let executor = pool.executor();
        let (tx, rx) = unbounded();
        let _release = gated(&executor, Lane::Scan, 1, tx.clone());
        assert_eq!(next(&rx), 1);
        let cancelled = Arc::new(AtomicBool::new(false));
        executor.submit(Lane::Cover, cancelled.clone(), move || {
            tx.send(2).unwrap();
        });
        cancelled.store(true, Ordering::Relaxed);
        executor.discard_cancelled();
        assert!(
            executor
                .0
                .state
                .lock()
                .unwrap()
                .queues
                .iter()
                .all(VecDeque::is_empty)
        );
        pool.shutdown();
        assert!(rx.try_recv().is_err());
        assert!(pool.workers.is_empty());
        assert!(!executor.submit(Lane::Scan, Arc::new(AtomicBool::new(false)), || {}));
        assert!(pool.prepare(0).is_err());
        assert!(Pool::new(AppSettings::max_concurrency() + 1).is_err());
    }
}
