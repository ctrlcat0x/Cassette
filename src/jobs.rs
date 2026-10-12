//! Bounded, nonblocking work mailboxes. New work replaces the oldest pending job.
//! In-flight work is owned by the receiver; sending never waits for I/O.
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{RecvError, RecvTimeoutError, SendError, TryRecvError},
    },
    time::Duration,
};
struct State<T> {
    pending: VecDeque<T>,
    senders: usize,
    receiving: bool,
}
struct Shared<T> {
    state: Mutex<State<T>>,
    ready: Condvar,
    capacity: usize,
}
pub struct JobSender<T>(Arc<Shared<T>>);
pub struct JobReceiver<T>(Arc<Shared<T>>);
pub fn channel<T>(capacity: usize) -> (JobSender<T>, JobReceiver<T>) {
    assert!(capacity > 0);
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            pending: VecDeque::new(),
            senders: 1,
            receiving: true,
        }),
        ready: Condvar::new(),
        capacity,
    });
    (JobSender(shared.clone()), JobReceiver(shared))
}
impl<T> JobSender<T> {
    pub fn send(&self, job: T) -> Result<(), SendError<T>> {
        let mut state = self.0.state.lock().unwrap();
        if !state.receiving {
            return Err(SendError(job));
        }
        if state.pending.len() == self.0.capacity {
            state.pending.pop_front();
        }
        state.pending.push_back(job);
        self.0.ready.notify_one();
        Ok(())
    }
}
impl<T> Clone for JobSender<T> {
    fn clone(&self) -> Self {
        self.0.state.lock().unwrap().senders += 1;
        Self(self.0.clone())
    }
}
impl<T> Drop for JobSender<T> {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().senders -= 1;
        self.0.ready.notify_one();
    }
}
impl<T> JobReceiver<T> {
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        let state = self.0.state.lock().unwrap();
        let (mut state, _) = self
            .0
            .ready
            .wait_timeout_while(state, timeout, |state| {
                state.pending.is_empty() && state.senders != 0
            })
            .unwrap();
        state.pending.pop_front().ok_or(if state.senders == 0 {
            RecvTimeoutError::Disconnected
        } else {
            RecvTimeoutError::Timeout
        })
    }
    pub fn recv(&self) -> Result<T, RecvError> {
        let mut state = self.0.state.lock().unwrap();
        loop {
            if let Some(job) = state.pending.pop_front() {
                return Ok(job);
            }
            if state.senders == 0 {
                return Err(RecvError);
            }
            state = self.0.ready.wait(state).unwrap();
        }
    }
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let mut state = self.0.state.lock().unwrap();
        state.pending.pop_front().ok_or(if state.senders == 0 {
            TryRecvError::Disconnected
        } else {
            TryRecvError::Empty
        })
    }
}
impl<T> Drop for JobReceiver<T> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.receiving = false;
        state.pending.clear();
    }
}
