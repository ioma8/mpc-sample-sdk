//! Run one owned value on a worker thread and take the result back safely.
//!
//! A mod's UI thread must never block on a render.  A [`Task`] owns the value,
//! moves it to a named worker, catches a panic and hands the value back once.
//! The handle is the only state the feature keeps; the thread, result slot and
//! completion flag live here.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, TryLockError};

/// What a task can report when it is taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskError {
    /// The worker has not finished yet.
    Running,
    /// The worker panicked; the value was dropped.
    Panicked,
    /// The backend rejected the work with this feature-defined code.
    Render(i32),
    /// A terminal result was already taken.
    AlreadyTaken,
}

/// One worker task.
pub struct Task<T> {
    shared: Arc<Shared<T>>,
}

/// The worker-visible half of a task.
struct Shared<T> {
    /// The feature's key, usually the screen the work belongs to.
    key: usize,
    /// The terminal outcome, once the worker is done.
    outcome: Mutex<Option<Outcome<T>>>,
    /// Whether a terminal outcome was taken.
    taken: AtomicBool,
}

/// A worker's terminal state.
enum Outcome<T> {
    /// The closure succeeded and the value is here.
    Done(T),
    /// The closure failed with a feature-defined code.
    Render(i32),
    /// The closure panicked.
    Panicked,
}

impl<T> Task<T> {
    /// Start `run` on a worker thread with `value` moved in.
    ///
    /// A panic in `run` is caught on builds that unwind and reported as
    /// [`TaskError::Panicked`].  A release build that uses `panic = "abort"`
    /// aborts before that can be observed; feature code must still not panic.  If the thread cannot be created, the
    /// task is immediately terminal with [`TaskError::Panicked`].
    pub fn spawn<F>(key: usize, mut value: T, run: F) -> Self
    where
        T: Send + 'static,
        F: FnOnce(&mut T) -> Result<(), i32> + Send + 'static,
    {
        let shared = Arc::new(Shared {
            key,
            outcome: Mutex::new(None),
            taken: AtomicBool::new(false),
        });
        let worker = Arc::clone(&shared);
        let spawn = std::thread::Builder::new()
            .name("mod-task".to_string())
            .spawn(move || {
                let outcome = match catch_unwind(AssertUnwindSafe(|| run(&mut value))) {
                    Ok(Ok(())) => Outcome::Done(value),
                    Ok(Err(code)) => Outcome::Render(code),
                    Err(_) => Outcome::Panicked,
                };
                *lock(&worker.outcome) = Some(outcome);
            });
        if spawn.is_err() {
            *lock(&shared.outcome) = Some(Outcome::Panicked);
        }
        Self { shared }
    }

    /// The key supplied at spawn time.
    #[must_use]
    pub fn key(&self) -> usize {
        self.shared.key
    }

    /// Take the terminal outcome, or report why it is not available.
    ///
    /// This never blocks and never joins the thread.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Running`] while the worker runs,
    /// [`TaskError::Panicked`] if it panicked, [`TaskError::Render`] if the
    /// backend rejected the work, or [`TaskError::AlreadyTaken`] after a
    /// terminal result was taken.
    pub fn take(&mut self) -> Result<T, TaskError> {
        if self.shared.taken.swap(true, Ordering::AcqRel) {
            return Err(TaskError::AlreadyTaken);
        }
        let mut slot = match self.shared.outcome.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => {
                self.shared.taken.store(false, Ordering::Release);
                return Err(TaskError::Running);
            }
        };
        let Some(outcome) = slot.take() else {
            self.shared.taken.store(false, Ordering::Release);
            return Err(TaskError::Running);
        };
        match outcome {
            Outcome::Done(value) => Ok(value),
            Outcome::Render(code) => Err(TaskError::Render(code)),
            Outcome::Panicked => Err(TaskError::Panicked),
        }
    }
}

/// Lock a task mutex, recovering a poisoned value rather than panicking.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::{Task, TaskError};
    use std::time::{Duration, Instant};

    fn wait<T>(task: &mut Task<T>) -> Result<T, TaskError> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match task.take() {
                Err(TaskError::Running) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                other => return other,
            }
        }
    }

    #[test]
    fn polling_a_locked_result_returns_running() {
        let mut task = Task::spawn(1, 7, |_| Ok(()));
        let shared = std::sync::Arc::clone(&task.shared);
        let guard = super::lock(&shared.outcome);
        assert_eq!(task.take(), Err(TaskError::Running));
        drop(guard);
        assert_eq!(wait(&mut task), Ok(7));
    }

    #[test]
    fn a_task_returns_its_value_once() {
        let mut task = Task::spawn(7, vec![1_u8, 2, 3], |value| {
            value.push(4);
            Ok(())
        });
        assert_eq!(task.key(), 7);
        assert_eq!(wait(&mut task), Ok(vec![1, 2, 3, 4]));
        assert_eq!(task.take(), Err(TaskError::AlreadyTaken));
    }

    #[test]
    fn a_backend_failure_keeps_its_code() {
        let mut task = Task::spawn(1, 0_u8, |_| Err(42));
        assert_eq!(wait(&mut task), Err(TaskError::Render(42)));
    }

    #[test]
    fn a_panic_is_contained() {
        let mut task = Task::spawn(1, 0_u8, |_| panic!("render panic"));
        assert_eq!(wait(&mut task), Err(TaskError::Panicked));
    }

    #[test]
    fn an_unfinished_task_is_running() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut task = Task::spawn(1, 0_u8, move |_| {
            started_tx.send(()).expect("started");
            release_rx.recv().ok();
            Ok(())
        });
        started_rx.recv().expect("worker started");
        assert_eq!(task.take(), Err(TaskError::Running));
        release_tx.send(()).expect("release");
        assert_eq!(wait(&mut task), Ok(0));
    }
}
