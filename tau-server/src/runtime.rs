//! Runtime primitives shared by the transport and the future typed protocol.
//!
//! These types deliberately contain no agent or permission policy.  They are
//! the lifetime/ownership boundary at which a protocol adapter can plug in.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use std::future::Future;
use tokio::sync::{Notify, oneshot};

pub type ClientId = u64;
pub type TurnId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitError {
    Interrupted,
    Cancelled,
}

#[derive(Clone)]
pub struct CancellationAuthority {
    inner: Arc<Mutex<HashMap<String, Arc<Cancel>>>>,
}

struct Cancel {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancellationAuthority {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    pub fn register<T: ToString>(&self, turn: T) -> CancellationHandle {
        let turn = turn.to_string();
        let cancel = Arc::new(Cancel {
            cancelled: AtomicBool::new(false),
            notify: Notify::new(),
        });
        self.inner.lock().unwrap().insert(turn, cancel.clone());
        CancellationHandle { cancel }
    }
    /// Any attached client may invoke this.  The caller is intentionally not
    /// accepted here: authorization belongs to the protocol adapter.
    pub fn cancel<T: ToString>(&self, turn: T) -> bool {
        let turn = turn.to_string();
        let Some(cancel) = self.inner.lock().unwrap().get(&turn).cloned() else {
            return false;
        };
        cancel.cancelled.store(true, Ordering::Release);
        cancel.notify.notify_waiters();
        true
    }
    pub fn remove<T: ToString>(&self, turn: T) {
        let turn = turn.to_string();
        self.inner.lock().unwrap().remove(&turn);
    }
    pub fn interrupt_all(&self) {
        let turns = self
            .inner
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for turn in turns {
            self.cancel(turn);
        }
    }
}
impl Default for CancellationAuthority {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct CancellationHandle {
    cancel: Arc<Cancel>,
}
impl CancellationHandle {
    pub fn is_cancelled(&self) -> bool {
        self.cancel.cancelled.load(Ordering::Acquire)
    }
    pub async fn cancelled(&self) {
        let notified = self.cancel.notify.notified();
        if !self.is_cancelled() {
            notified.await;
        }
    }
}

struct Turn<T> {
    id: TurnId,
    value: T,
    done: Option<oneshot::Sender<Result<(), WaitError>>>,
    cancelled: Arc<AtomicBool>,
}

struct ActiveState {
    id: TurnId,
    cancelled: Arc<AtomicBool>,
}

/// One active turn per session; admission order is assigned by the server.
pub struct SessionTurnQueue<T> {
    next: AtomicU64,
    state: Mutex<(Option<ActiveState>, VecDeque<Turn<T>>)>,
    wake: Notify,
}
impl<T> SessionTurnQueue<T> {
    pub fn new() -> Self {
        Self {
            next: AtomicU64::new(1),
            state: Mutex::new((None, VecDeque::new())),
            wake: Notify::new(),
        }
    }
    pub fn submit(&self, value: T) -> (TurnId, oneshot::Receiver<Result<(), WaitError>>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.state.lock().unwrap().1.push_back(Turn {
            id,
            value,
            done: Some(tx),
            cancelled,
        });
        self.wake.notify_one();
        (id, rx)
    }
    pub async fn next(&self) -> ActiveTurn<'_, T> {
        loop {
            if let Some(turn) = {
                let mut state = self.state.lock().unwrap();
                if state.0.is_none() && !state.1.is_empty() {
                    let turn = state.1.pop_front();
                    if let Some(turn) = &turn {
                        state.0 = Some(ActiveState {
                            id: turn.id,
                            cancelled: turn.cancelled.clone(),
                        });
                    }
                    turn
                } else {
                    None
                }
            } {
                return ActiveTurn {
                    queue: self,
                    turn: Some(turn),
                };
            }
            self.wake.notified().await;
        }
    }

    /// Run one admitted item while retaining the queue's active slot.  This is
    /// the production-facing form of the queue: callers cannot accidentally
    /// release the slot before the terminal result has been produced.
    pub async fn run_next<F, Fut>(&self, run: F)
    where
        T: Clone,
        F: FnOnce(T) -> Fut,
        Fut: Future<Output = ()>,
    {
        let active = self.next().await;
        let value = active.value().clone();
        run(value).await;
        active.finish(Ok(()));
    }
    fn finish(&self, turn: Turn<T>, result: Result<(), WaitError>) {
        let result = if turn.cancelled.load(Ordering::Acquire) {
            Err(WaitError::Cancelled)
        } else {
            result
        };
        if let Some(done) = turn.done {
            let _ = done.send(result);
        }
        let mut state = self.state.lock().unwrap();
        // Only the currently admitted turn may release the slot.  This keeps
        // cancellation from allowing a second turn to run concurrently.
        if state.0.as_ref().is_some_and(|active| active.id == turn.id) {
            state.0 = None;
        }
        drop(state);
        self.wake.notify_one();
    }

    /// Cancel a turn without admitting another one.  Queued work is settled
    /// immediately; admitted work remains active until its executor drops or
    /// explicitly finishes its guard.
    pub fn cancel(&self, id: TurnId) -> bool {
        let mut state = self.state.lock().unwrap();
        if let Some(active) = state.0.as_ref().filter(|active| active.id == id) {
            active.cancelled.store(true, Ordering::Release);
            return true;
        }
        let Some(position) = state.1.iter().position(|turn| turn.id == id) else {
            return false;
        };
        let mut turn = state.1.remove(position).expect("position was found");
        turn.cancelled.store(true, Ordering::Release);
        if let Some(done) = turn.done.take() {
            let _ = done.send(Err(WaitError::Cancelled));
        }
        true
    }
}
impl<T> Default for SessionTurnQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for SessionTurnQueue<T> {
    fn drop(&mut self) {
        // An ActiveTurn borrows the queue, so only queued items can remain at
        // this point.  Do not strand their admission waiters.
        if let Ok(state) = self.state.get_mut() {
            for mut turn in state.1.drain(..) {
                turn.cancelled.store(true, Ordering::Release);
                if let Some(done) = turn.done.take() {
                    let _ = done.send(Err(WaitError::Interrupted));
                }
            }
        }
    }
}

pub struct ActiveTurn<'a, T> {
    queue: &'a SessionTurnQueue<T>,
    turn: Option<Turn<T>>,
}
impl<T> ActiveTurn<'_, T> {
    pub fn id(&self) -> TurnId {
        self.turn.as_ref().unwrap().id
    }
    pub fn value(&self) -> &T {
        &self.turn.as_ref().unwrap().value
    }
    pub fn finish(mut self, result: Result<(), WaitError>) {
        self.queue.finish(self.turn.take().unwrap(), result);
    }
}
impl<T> Drop for ActiveTurn<'_, T> {
    fn drop(&mut self) {
        if let Some(turn) = self.turn.take() {
            self.queue.finish(turn, Err(WaitError::Interrupted));
        }
    }
}

#[derive(Clone)]
pub struct EventLog<E> {
    next: Arc<AtomicU64>,
    events: Arc<Mutex<VecDeque<(u64, E)>>>,
    capacity: usize,
    broadcast: tokio::sync::broadcast::Sender<(u64, E)>,
}
impl<E: Clone + Send + 'static> EventLog<E> {
    pub fn new(capacity: usize) -> Self {
        let (broadcast, _) = tokio::sync::broadcast::channel(capacity.max(1));
        Self {
            next: Arc::new(AtomicU64::new(1)),
            events: Arc::new(Mutex::new(VecDeque::new())),
            capacity: capacity.max(1),
            broadcast,
        }
    }
    pub fn append(&self, event: E) -> u64 {
        let seq = self.next.fetch_add(1, Ordering::Relaxed);
        let mut events = self.events.lock().unwrap();
        events.push_back((seq, event.clone()));
        while events.len() > self.capacity {
            events.pop_front();
        }
        let _ = self.broadcast.send((seq, event));
        seq
    }
    pub fn replay_since(&self, sequence: u64) -> Vec<(u64, E)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(seq, _)| *seq > sequence)
            .cloned()
            .collect()
    }
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<(u64, E)> {
        self.broadcast.subscribe()
    }
}

#[derive(Clone)]
pub struct ConnectionRegistry {
    next: Arc<AtomicU64>,
    clients: Arc<Mutex<HashMap<ClientId, ()>>>,
}
impl ConnectionRegistry {
    pub fn new() -> Self {
        Self {
            next: Arc::new(AtomicU64::new(1)),
            clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    pub fn attach(&self) -> ClientId {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.clients.lock().unwrap().insert(id, ());
        id
    }
    pub fn detach(&self, id: ClientId) -> bool {
        self.clients.lock().unwrap().remove(&id).is_some()
    }
    pub fn is_attached(&self, id: ClientId) -> bool {
        self.clients.lock().unwrap().contains_key(&id)
    }
    pub fn len(&self) -> usize {
        self.clients.lock().unwrap().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
impl Default for ConnectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// A waitable prompt.  Answering is deliberately separate from ownership:
/// questions accept any attached client, while adapters may require `owner`.
pub struct InputWait<T> {
    result: Mutex<Option<Result<T, WaitError>>>,
    notify: Notify,
    owner: ClientId,
}
impl<T> InputWait<T> {
    pub fn new(owner: ClientId) -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(None),
            notify: Notify::new(),
            owner,
        })
    }
    pub fn owner(&self) -> ClientId {
        self.owner
    }
    pub fn answer(&self, value: T) -> bool {
        let mut result = self.result.lock().unwrap();
        if result.is_some() {
            return false;
        }
        *result = Some(Ok(value));
        self.notify.notify_one();
        true
    }
    pub fn interrupt(&self) {
        let mut result = self.result.lock().unwrap();
        if result.is_none() {
            *result = Some(Err(WaitError::Interrupted));
            self.notify.notify_one();
        }
    }
    pub async fn wait(&self) -> Result<T, WaitError> {
        loop {
            let notified = self.notify.notified();
            if let Some(result) = self.result.lock().unwrap().take() {
                return result;
            }
            notified.await;
        }
    }
}

/// Process-lifetime boundary.  Dropping/recreating this value is the restart
/// boundary: callers interrupt live turns and waits before restoring history.
#[derive(Clone)]
pub struct Runtime {
    pub connections: ConnectionRegistry,
    pub cancellation: CancellationAuthority,
    interrupted: Arc<AtomicBool>,
}
impl Runtime {
    pub fn new() -> Self {
        Self {
            connections: ConnectionRegistry::new(),
            cancellation: CancellationAuthority::new(),
            interrupted: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn interrupt_for_restart(&self) {
        self.interrupted.store(true, Ordering::Release);
        self.cancellation.interrupt_all();
    }
    pub fn is_restarted(&self) -> bool {
        self.interrupted.load(Ordering::Acquire)
    }
}
impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn queue_is_fifo_and_only_has_one_active_turn() {
        let queue = Arc::new(SessionTurnQueue::new());
        let (_, first_done) = queue.submit("first");
        let (_, second_done) = queue.submit("second");
        let active = queue.next().await;
        assert_eq!(active.value(), &"first");
        active.finish(Ok(()));
        let active = queue.next().await;
        assert_eq!(active.value(), &"second");
        active.finish(Ok(()));
        assert_eq!(first_done.await.unwrap(), Ok(()));
        assert_eq!(second_done.await.unwrap(), Ok(()));
    }

    #[tokio::test]
    async fn cancellation_settles_queued_turn_without_releasing_active_slot() {
        let queue = Arc::new(SessionTurnQueue::new());
        let (first, first_done) = queue.submit("first");
        let (second, second_done) = queue.submit("second");
        let active = queue.next().await;
        assert!(queue.cancel(second));
        assert_eq!(second_done.await.unwrap(), Err(WaitError::Cancelled));
        assert!(queue.cancel(first));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), queue.next())
                .await
                .is_err()
        );
        active.finish(Ok(()));
        assert_eq!(first_done.await.unwrap(), Err(WaitError::Cancelled));
    }

    #[tokio::test]
    async fn wait_is_indefinite_until_answered_and_restart_interrupts_cancellation() {
        let wait = InputWait::new(7);
        assert!(wait.answer(42));
        assert_eq!(wait.wait().await, Ok(42));

        let runtime = Runtime::new();
        let handle = runtime.cancellation.register(9);
        runtime.interrupt_for_restart();
        assert!(runtime.is_restarted());
        assert!(handle.is_cancelled());
    }

    #[test]
    fn event_replay_and_connection_lifecycle_are_independent() {
        let events = EventLog::new(2);
        events.append("a");
        events.append("b");
        events.append("c");
        assert_eq!(events.replay_since(1), vec![(2, "b"), (3, "c")]);
        let registry = ConnectionRegistry::new();
        let id = registry.attach();
        assert_eq!(registry.len(), 1);
        assert!(registry.detach(id));
        assert_eq!(registry.len(), 0);
    }
}
