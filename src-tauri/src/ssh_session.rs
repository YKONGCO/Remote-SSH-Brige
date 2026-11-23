use crate::model::{RuntimeSettings, Server};
use anyhow::Result;
use serde_json::{json, Value};
use ssh2::Session;
use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

pub const MAX_FOREGROUND: usize = 2;

pub const MAX_JOBS: usize = 4;
pub const MAX_CONNECTIONS: usize = MAX_FOREGROUND + MAX_JOBS;
pub const MAX_IDLE: usize = 2;
pub const IDLE_TTL: Duration = Duration::from_secs(60);
pub const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct SessionManager {
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
    foreground: usize,
    jobs: usize,
    connecting: usize,
    last_error: Option<String>,
    counters: Counters,
}

struct Pool {
    state: Mutex<PoolState>,
    changed: Condvar,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LeaseRole {
    Foreground,
    Job,
}

struct LeaseCore {
    pool: Arc<Pool>,
    signature: String,
    role: Mutex<LeaseRole>,
    reused: bool,
}

#[derive(Clone)]
pub struct SessionHandle {
    core: Arc<LeaseCore>,
}

impl SessionHandle {
    pub fn promote_to_job(&self) -> bool {
        let mut role = self.core.role.lock().unwrap();
        if *role != LeaseRole::Foreground {
            return false;
        }
        let mut state = self.core.pool.state.lock().unwrap();
        if state.foreground == 0 {
            return false;
        }
        state.foreground -= 1;
        state.jobs += 1;
        *role = LeaseRole::Job;
        self.core.pool.changed.notify_all();
        true
    }

    pub fn reused(&self) -> bool {
        self.core.reused
    }
}

pub struct SessionLease {
    core: Arc<LeaseCore>,
    session: Option<Session>,
    released: bool,
}

impl SessionLease {
    pub fn session(&mut self) -> &mut Session {
        self.session
            .as_mut()
            .expect("session lease is always populated before release")
    }

    pub fn handle(&self) -> SessionHandle {
        SessionHandle {
            core: self.core.clone(),
        }
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
        let role = *self.core.role.lock().unwrap();
        let pool = self.core.pool.clone();
        let mut state = pool.state.lock().unwrap();
        SessionManager::prune_idle(&mut state, &self.core.signature);
        match role {
            LeaseRole::Foreground => state.foreground = state.foreground.saturating_sub(1),
            LeaseRole::Job => state.jobs = state.jobs.saturating_sub(1),
        }
        if healthy && session.authenticated() && state.idle.len() < MAX_IDLE {
            session.set_blocking(true);
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
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        self.release_inner(false);
    }
}

impl SessionManager {
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

    pub fn acquire(&self, server: &Server, settings: &RuntimeSettings) -> Result<SessionLease> {
        let signature = crate::ssh::connection_signature(server);
        let pool = self.pool(&server.id);
        let deadline = Instant::now() + ACQUIRE_TIMEOUT;
        let mut channel_failures = 0;
        loop {
            enum Step {
                Reuse(Session),
                Connect,
                Wait,
            }
            let step = {
                let mut state = pool.state.lock().unwrap();
                Self::prune_idle(&mut state, &signature);
                if state.foreground + state.connecting >= MAX_FOREGROUND {
                    Step::Wait
                } else if let Some(idle) = state.idle.pop() {
                    state.foreground += 1;
                    Step::Reuse(idle.session)
                } else {
                    if state.idle.len() + state.foreground + state.jobs + state.connecting
                        >= MAX_CONNECTIONS
                    {
                        if let Some(idle) = state.idle.pop() {
                            state.counters.discarded += 1;
                            drop(idle);
                        }
                    }
                    if state.idle.len() + state.foreground + state.jobs + state.connecting
                        >= MAX_CONNECTIONS
                    {
                        Step::Wait
                    } else {
                        state.connecting += 1;
                        Step::Connect
                    }
                }
            };
            match step {
                Step::Reuse(session) => {
                    let healthy = session.channel_session().is_ok();
                    let mut state = pool.state.lock().unwrap();
                    if healthy {
                        state.counters.reused += 1;
                        drop(state);
                        return Ok(Self::lease(pool.clone(), signature.clone(), session, true));
                    }
                    state.foreground = state.foreground.saturating_sub(1);
                    state.counters.discarded += 1;
                    state.last_error = Some("Idle SSH session could not open a channel".into());
                    pool.changed.notify_all();
                    drop(state);
                    channel_failures += 1;
                    if channel_failures > 2 || Instant::now() >= deadline {
                        return Err(crate::operation_error::error(
                            "SSH_CHANNEL_FAILED",
                            "No healthy SSH session could be reused",
                        ));
                    }
                    continue;
                }
                Step::Connect => {
                    let result = connect_with_retry(server, settings);
                    let mut state = pool.state.lock().unwrap();
                    state.connecting = state.connecting.saturating_sub(1);
                    match result {
                        Ok(session) => {
                            state.foreground += 1;
                            state.counters.created += 1;
                            pool.changed.notify_all();
                            drop(state);
                            return Ok(Self::lease(
                                pool.clone(),
                                signature.clone(),
                                session,
                                false,
                            ));
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
                            "No SSH session became available; retry after a running command finishes",
                            json!({
                                "foreground": state.foreground,
                                "jobs": state.jobs,
                                "idle": state.idle.len(),
                                "max_foreground": MAX_FOREGROUND,
                                "max_connections": MAX_CONNECTIONS,
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

    fn lease(pool: Arc<Pool>, signature: String, session: Session, reused: bool) -> SessionLease {
        SessionLease {
            core: Arc::new(LeaseCore {
                pool,
                signature,
                role: Mutex::new(LeaseRole::Foreground),
                reused,
            }),
            session: Some(session),
            released: false,
        }
    }

    fn prune_idle(state: &mut PoolState, signature: &str) {
        let before = state.idle.len();
        state
            .idle
            .retain(|idle| idle.signature == signature && idle.since.elapsed() < IDLE_TTL);
        state.counters.discarded += (before - state.idle.len()) as u64;
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
                "idle": 0, "foreground": 0, "jobs": 0, "connecting": 0,
                "created": 0, "reused": 0, "failed": 0, "discarded": 0, "timed_out": 0,
                "last_error": null,
                "limits": {
                    "max_foreground": MAX_FOREGROUND,
                    "max_jobs": MAX_JOBS,
                    "max_connections": MAX_CONNECTIONS,
                    "max_idle": MAX_IDLE,
                    "idle_ttl_secs": IDLE_TTL.as_secs(),
                },
            });
        };
        let state = pool.state.lock().unwrap();
        json!({
            "idle": state.idle.len(),
            "foreground": state.foreground,
            "jobs": state.jobs,
            "connecting": state.connecting,
            "created": state.counters.created,
            "reused": state.counters.reused,
            "failed": state.counters.failed,
            "discarded": state.counters.discarded,
            "timed_out": state.counters.timed_out,
            "last_error": state.last_error,
            "limits": {
                "max_foreground": MAX_FOREGROUND,
                "max_jobs": MAX_JOBS,
                "max_connections": MAX_CONNECTIONS,
                "max_idle": MAX_IDLE,
                "idle_ttl_secs": IDLE_TTL.as_secs(),
            },
        })
    }
}

fn connect_with_retry(server: &Server, settings: &RuntimeSettings) -> Result<Session> {
    let mut attempt = 0u32;
    loop {
        let _slot = crate::ssh::connect_slot(server)?;
        match crate::ssh::connect_with_settings(server, settings) {
            Ok(session) => return Ok(session),
            Err(error) => {
                if attempt >= 2 || !crate::operation_error::retryable(&error) {
                    return Err(error.context("Cannot establish SSH session"));
                }
                attempt += 1;
                let base = 250u64 << (attempt - 1);
                let jitter = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos() as u64 % 150)
                    .unwrap_or(0);
                std::thread::sleep(Duration::from_millis(base + jitter));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_change_with_connection_identity_but_not_with_name() {
        let mut server = Server {
            id: "s1".into(),
            host: "a.example".into(),
            port: 22,
            username: "root".into(),
            auth: "key".into(),
            key_path: "/k".into(),
            fingerprint: "SHA256:x".into(),
            name: "prod".into(),
            ..Default::default()
        };
        let original = crate::ssh::connection_signature(&server);
        server.name = "renamed".into();
        assert_eq!(original, crate::ssh::connection_signature(&server));
        server.username = "deploy".into();
        assert_ne!(original, crate::ssh::connection_signature(&server));
        server.username = "root".into();
        server.port = 2222;
        assert_ne!(original, crate::ssh::connection_signature(&server));
    }

    #[test]
    fn idle_pruning_respects_signature_and_ttl() {
        let mut state = PoolState::default();
        state.idle.push(IdleSession {
            session: Session::new().unwrap(),
            since: Instant::now(),
            signature: "one".into(),
        });
        SessionManager::prune_idle(&mut state, "one");
        assert_eq!(state.idle.len(), 1);
        SessionManager::prune_idle(&mut state, "two");
        assert!(state.idle.is_empty());
        assert_eq!(state.counters.discarded, 1);
    }
}
