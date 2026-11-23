use crate::model::{RuntimeSettings, Server};
use anyhow::Result;
use serde_json::{json, Value};
use ssh2::Session;
use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

const MAX_IDLE: usize = 2;
const IDLE_TTL: Duration = Duration::from_secs(60);
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct SftpSessionManager {
    pools: Mutex<HashMap<String, Arc<Pool>>>,
}

struct IdleSession {
    session: Session,
    since: Instant,
    signature: String,
}

#[derive(Default)]
struct Counters {
    created: u64,
    reused: u64,
    failed: u64,
    discarded: u64,
    timed_out: u64,
}

#[derive(Default)]
struct PoolState {
    idle: Vec<IdleSession>,
    active: usize,
    connecting: usize,
    last_error: Option<String>,
    counters: Counters,
}

struct Pool {
    state: Mutex<PoolState>,
    changed: Condvar,
}

struct LeaseCore {
    pool: Arc<Pool>,
    signature: String,
    reused: bool,
}

pub struct SftpLease {
    core: Arc<LeaseCore>,
    session: Option<Session>,
    released: bool,
}

impl SftpLease {
    pub fn session(&mut self) -> &mut Session {
        self.session
            .as_mut()
            .expect("SFTP lease is always populated before release")
    }

    pub fn reused(&self) -> bool {
        self.core.reused
    }

    pub fn release(mut self, healthy: bool) {
        self.release_inner(healthy);
    }

    fn release_inner(&mut self, healthy: bool) {
        if self.released {
            return;
        }
        self.released = true;
        let Some(session) = self.session.take() else {
            return;
        };
        let pool = self.core.pool.clone();
        let mut state = pool.state.lock().unwrap();
        state.active = state.active.saturating_sub(1);
        Self::prune_locked(&mut state, &self.core.signature);
        if healthy && session.authenticated() && state.idle.len() < MAX_IDLE {
            state.idle.push(IdleSession {
                session,
                since: Instant::now(),
                signature: self.core.signature.clone(),
            });
        } else {
            state.counters.discarded += 1;
        }
        pool.changed.notify_all();
    }

    fn prune_locked(state: &mut PoolState, signature: &str) {
        let before = state.idle.len();
        state
            .idle
            .retain(|idle| idle.signature == signature && idle.since.elapsed() < IDLE_TTL);
        state.counters.discarded += (before - state.idle.len()) as u64;
    }
}

impl Drop for SftpLease {
    fn drop(&mut self) {
        self.release_inner(false);
    }
}

impl SftpSessionManager {
    fn pool(&self, server_id: &str) -> Arc<Pool> {
        let mut pools = self.pools.lock().unwrap();
        pools
            .entry(server_id.to_string())
            .or_insert_with(|| {
                Arc::new(Pool {
                    state: Mutex::new(PoolState::default()),
                    changed: Condvar::new(),
                })
            })
            .clone()
    }

    pub fn acquire(&self, server: &Server, settings: &RuntimeSettings) -> Result<SftpLease> {
        let signature = crate::ssh::connection_signature(server);
        let pool = self.pool(&server.id);
        let max_connections = settings
            .transfers
            .per_server_concurrent
            .saturating_add(2)
            .clamp(2, 10);
        let deadline = Instant::now() + ACQUIRE_TIMEOUT;
        loop {
            enum Step {
                Reuse(Session),
                Connect,
                Wait,
            }
            let step = {
                let mut state = pool.state.lock().unwrap();
                SftpLease::prune_locked(&mut state, &signature);
                if let Some(idle) = state.idle.pop() {
                    state.active += 1;
                    Step::Reuse(idle.session)
                } else if state.active + state.idle.len() + state.connecting < max_connections {
                    state.connecting += 1;
                    Step::Connect
                } else {
                    Step::Wait
                }
            };
            match step {
                Step::Reuse(session) => {
                    let healthy = session.sftp().is_ok();
                    let mut state = pool.state.lock().unwrap();
                    if healthy {
                        state.counters.reused += 1;
                        drop(state);
                        return Ok(SftpLease {
                            core: Arc::new(LeaseCore {
                                pool: pool.clone(),
                                signature,
                                reused: true,
                            }),
                            session: Some(session),
                            released: false,
                        });
                    }
                    state.active = state.active.saturating_sub(1);
                    state.counters.discarded += 1;
                    state.last_error = Some("Idle SFTP session is no longer usable".into());
                    pool.changed.notify_all();
                }
                Step::Connect => {
                    let result = {
                        let _slot = crate::ssh::connect_slot(server);
                        let _slot = match _slot {
                            Ok(slot) => slot,
                            Err(error) => {
                                let mut state = pool.state.lock().unwrap();
                                state.connecting = state.connecting.saturating_sub(1);
                                state.counters.failed += 1;
                                state.last_error =
                                    Some(crate::operation_error::error_message(&error));
                                pool.changed.notify_all();
                                return Err(error);
                            }
                        };
                        crate::ssh::connect_with_settings(server, settings)
                    };
                    let mut state = pool.state.lock().unwrap();
                    state.connecting = state.connecting.saturating_sub(1);
                    match result {
                        Ok(session) => {
                            state.active += 1;
                            state.counters.created += 1;
                            pool.changed.notify_all();
                            drop(state);
                            return Ok(SftpLease {
                                core: Arc::new(LeaseCore {
                                    pool: pool.clone(),
                                    signature,
                                    reused: false,
                                }),
                                session: Some(session),
                                released: false,
                            });
                        }
                        Err(error) => {
                            state.counters.failed += 1;
                            state.last_error = Some(crate::operation_error::error_message(&error));
                            pool.changed.notify_all();
                            return Err(error);
                        }
                    }
                }
                Step::Wait => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        let mut state = pool.state.lock().unwrap();
                        state.counters.timed_out += 1;
                        return Err(crate::operation_error::error_with_details(
                            "SSH_SESSION_BUSY",
                            "No SFTP session became available; retry later",
                            json!({
                                "active": state.active,
                                "connecting": state.connecting,
                                "idle": state.idle.len(),
                                "max_connections": max_connections,
                            }),
                        ));
                    }
                    let state = pool.state.lock().unwrap();
                    let _ = pool
                        .changed
                        .wait_timeout(state, remaining.min(Duration::from_millis(500)))
                        .unwrap();
                }
            }
        }
    }

    pub fn prune(&self) {
        let pools: Vec<_> = self.pools.lock().unwrap().values().cloned().collect();
        for pool in pools {
            let mut state = pool.state.lock().unwrap();
            let before = state.idle.len();
            state.idle.retain(|idle| idle.since.elapsed() < IDLE_TTL);
            state.counters.discarded += (before - state.idle.len()) as u64;
            if state.idle.len() != before {
                pool.changed.notify_all();
            }
        }
    }

    pub fn remove(&self, server_id: &str) {
        self.pools.lock().unwrap().remove(server_id);
    }

    pub fn diagnostics(&self, server_id: &str) -> Value {
        let pools = self.pools.lock().unwrap();
        let Some(pool) = pools.get(server_id) else {
            return json!({
                "idle": 0, "active": 0, "connecting": 0,
                "created": 0, "reused": 0, "failed": 0, "discarded": 0, "timed_out": 0,
                "last_error": null,
                "limits": { "max_idle": MAX_IDLE, "idle_ttl_secs": IDLE_TTL.as_secs() },
            });
        };
        let state = pool.state.lock().unwrap();
        json!({
            "idle": state.idle.len(),
            "active": state.active,
            "connecting": state.connecting,
            "created": state.counters.created,
            "reused": state.counters.reused,
            "failed": state.counters.failed,
            "discarded": state.counters.discarded,
            "timed_out": state.counters.timed_out,
            "last_error": state.last_error,
            "limits": { "max_idle": MAX_IDLE, "idle_ttl_secs": IDLE_TTL.as_secs() },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_pruning_respects_signature_and_reports_empty_diagnostics() {
        let mut state = PoolState::default();
        state.idle.push(IdleSession {
            session: Session::new().unwrap(),
            since: Instant::now(),
            signature: "one".into(),
        });
        SftpLease::prune_locked(&mut state, "one");
        assert_eq!(state.idle.len(), 1);
        SftpLease::prune_locked(&mut state, "two");
        assert!(state.idle.is_empty());
        assert_eq!(state.counters.discarded, 1);
        let manager = SftpSessionManager::default();
        let diagnostics = manager.diagnostics("missing");
        assert_eq!(diagnostics["created"], 0);
        assert_eq!(diagnostics["active"], 0);
    }
}
