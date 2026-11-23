use crate::{transfers::Handle, transfers::TransferSettings};
use anyhow::Result;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::Notify;

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub max_concurrent: usize,
    pub per_server: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            per_server: 2,
        }
    }
}

struct Queued {
    endpoint: String,
    priority: i32,
    sequence: u64,
    enqueued: Instant,
}

#[derive(Default)]
struct State {
    queued: HashMap<String, Queued>,
    running: HashMap<String, String>,
    sequence: u64,
}

pub(crate) struct TransferScheduler {
    state: Mutex<State>,
    limits: Mutex<Limits>,
    wake: Notify,
}

pub(crate) struct RunPermit {
    scheduler: Arc<TransferScheduler>,
    id: String,
}

struct QueueGuard {
    scheduler: Arc<TransferScheduler>,
    id: String,
}

impl Drop for QueueGuard {
    fn drop(&mut self) {
        let removed = self
            .scheduler
            .state
            .lock()
            .unwrap()
            .queued
            .remove(&self.id)
            .is_some();
        if removed {
            self.scheduler.wake.notify_waiters();
        }
    }
}

impl RunPermit {
    #[cfg(test)]
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
}

impl Drop for RunPermit {
    fn drop(&mut self) {
        {
            let mut state = self.scheduler.state.lock().unwrap();
            state.running.remove(&self.id);
        }
        self.scheduler.wake.notify_waiters();
    }
}

impl TransferScheduler {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            limits: Mutex::new(Limits::default()),
            wake: Notify::new(),
        })
    }

    pub(crate) fn configure(&self, settings: &TransferSettings) {
        *self.limits.lock().unwrap() = Limits {
            max_concurrent: settings.max_concurrent,
            per_server: settings.per_server_concurrent,
        };
        self.wake.notify_waiters();
    }

    pub(crate) fn cancel(&self, id: &str) {
        {
            let mut state = self.state.lock().unwrap();
            state.queued.remove(id);
        }
        self.wake.notify_waiters();
    }

    pub(crate) fn cancel_all(&self) {
        self.state.lock().unwrap().queued.clear();
        self.wake.notify_waiters();
    }

    pub(crate) fn update_priority(&self, id: &str, priority: i32) {
        {
            let mut state = self.state.lock().unwrap();
            if let Some(queued) = state.queued.get_mut(id) {
                queued.priority = priority;
            }
        }
        self.wake.notify_waiters();
    }

    #[cfg(test)]
    fn queued_priorities(&self) -> Vec<(String, i32)> {
        self.state
            .lock()
            .unwrap()
            .queued
            .iter()
            .map(|(id, queued)| (id.clone(), queued.priority))
            .collect()
    }

    fn dispatch(self: &Arc<Self>, state: &mut State, id: &str) -> Option<RunPermit> {
        let limits = *self.limits.lock().unwrap();
        if state.running.len() >= limits.max_concurrent {
            return None;
        }
        let own = state.queued.get(id)?;
        if state
            .running
            .values()
            .filter(|endpoint| *endpoint == &own.endpoint)
            .count()
            >= limits.per_server
        {
            return None;
        }
        let mut best: Option<(&str, i64, u64)> = None;
        for (key, queued) in &state.queued {
            if state
                .running
                .values()
                .filter(|endpoint| *endpoint == &queued.endpoint)
                .count()
                >= limits.per_server
            {
                continue;
            }
            let waited = queued.enqueued.elapsed().as_secs().min(3600) as i64;
            let score = queued.priority as i64 * 10 + waited;
            let better = best.is_none_or(|(_, best_score, best_sequence)| {
                score > best_score || (score == best_score && queued.sequence < best_sequence)
            });
            if better {
                best = Some((key.as_str(), score, queued.sequence));
            }
        }
        if best.map(|(key, _, _)| key) != Some(id) {
            return None;
        }
        let endpoint = own.endpoint.clone();
        state.queued.remove(id);
        state.running.insert(id.to_string(), endpoint);
        Some(RunPermit {
            scheduler: self.clone(),
            id: id.to_string(),
        })
    }

    pub(crate) async fn acquire(
        self: &Arc<Self>,
        id: &str,
        endpoint: &str,
        priority: i32,
        handle: &Handle,
    ) -> Result<RunPermit> {
        let _guard = QueueGuard {
            scheduler: self.clone(),
            id: id.to_string(),
        };
        let mut registered = false;
        loop {
            handle.check_cancelled()?;
            {
                let mut state = self.state.lock().unwrap();
                if !registered {
                    state.sequence += 1;
                    let sequence = state.sequence;
                    state.queued.insert(
                        id.to_string(),
                        Queued {
                            endpoint: endpoint.to_string(),
                            priority,
                            sequence,
                            enqueued: Instant::now(),
                        },
                    );
                    registered = true;
                }
                if let Some(permit) = self.dispatch(&mut state, id) {
                    return Ok(permit);
                }
            }
            let notified = self.wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = self.state.lock().unwrap();
                if !state.queued.contains_key(id) {
                    registered = false;
                    drop(state);
                    handle.check_cancelled()?;
                    continue;
                }
                if let Some(permit) = self.dispatch(&mut state, id) {
                    return Ok(permit);
                }
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn settings() -> TransferSettings {
        TransferSettings {
            max_concurrent: 1,
            per_server_concurrent: 1,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_released_slot_is_handed_to_the_next_waiter() {
        let scheduler = TransferScheduler::new();
        scheduler.configure(&settings());
        let first = crate::transfers::testing_handle();
        let second = crate::transfers::testing_handle();
        let permit = scheduler
            .acquire("first", "endpoint", 0, &first)
            .await
            .unwrap();
        let waiter = scheduler.clone();
        let mut waiting = tokio::spawn(async move {
            waiter
                .acquire("second", "endpoint", 0, &second)
                .await
                .map(|permit| permit.id().to_string())
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut waiting)
                .await
                .is_err()
        );
        drop(permit);
        let result = tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result, "second");
    }

    #[tokio::test]
    async fn priority_selects_the_next_waiter() {
        let scheduler = TransferScheduler::new();
        scheduler.configure(&settings());
        let holder = crate::transfers::testing_handle();
        let low = crate::transfers::testing_handle();
        let high = crate::transfers::testing_handle();
        let permit = scheduler
            .acquire("holder", "endpoint", 0, &holder)
            .await
            .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let low_scheduler = scheduler.clone();
        let low_tx = tx.clone();
        let low_task = tokio::spawn(async move {
            let permit = low_scheduler
                .acquire("low", "endpoint", 0, &low)
                .await
                .unwrap();
            low_tx.send(permit.id().to_string()).unwrap();
            tokio::time::sleep(Duration::from_secs(2)).await;
            drop(permit);
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let high_scheduler = scheduler.clone();
        let high_task = tokio::spawn(async move {
            let permit = high_scheduler
                .acquire("high", "endpoint", 5, &high)
                .await
                .unwrap();
            tx.send(permit.id().to_string()).unwrap();
            tokio::time::sleep(Duration::from_secs(2)).await;
            drop(permit);
        });
        for _ in 0..100 {
            if scheduler
                .queued_priorities()
                .iter()
                .any(|(id, priority)| id == "high" && *priority == 5)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        drop(permit);
        let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first, "high");
        low_task.abort();
        high_task.abort();
    }
}
