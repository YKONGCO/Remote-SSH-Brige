// Persistent task lifecycle. Included in transfers.rs to keep manager internals private.
impl Handle {
    pub(crate) fn validate_server(&self, server: &Server) -> Result<()> {
        anyhow::ensure!(
            self.server_identity
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|original| serde_json::to_value(original).ok()
                    == serde_json::to_value(server).ok()),
            "Server configuration changed; inspect retained temporary files manually"
        );
        Ok(())
    }
    pub(crate) fn retry_count(&self) -> u32 {
        self.snapshot.lock().unwrap().retry_count
    }
    pub(crate) fn persist(&self) -> Result<()> {
        let _serial = self.journal_lock.lock().unwrap();
        let Some(state) = self.state.lock().unwrap().upgrade() else {
            return Ok(());
        };
        let record = json!({
            "snapshot":self.snapshot(), "operation":self.operation.lock().unwrap().clone(),
            "origin":self.origin.lock().unwrap().clone(),
            "checkpoint":self.checkpoint.lock().unwrap().clone(),
            "server":self.server_identity.lock().unwrap().clone(),
        });
        let id = record["snapshot"]["transfer_id"]
            .as_str()
            .context("Missing transfer id")?;
        let result = state.database.lock().unwrap().save_transfer(id, &record);
        result
    }
    pub(crate) fn phase(&self, phase: &str) {
        self.update(|s| s.phase = phase.into());
    }
    pub(crate) fn report(&self, bytes: u64, total: u64, started: Instant, initial: u64) {
        self.progress(bytes, total, started.elapsed());
        self.update(|s| {
            s.bytes_per_second = ((bytes.saturating_sub(initial)) as f64
                / started.elapsed().as_secs_f64().max(0.001))
                as u64
        });
    }
    pub(crate) fn bandwidth(&self, settings: &TransferSettings) -> u64 {
        let task = self.snapshot.lock().unwrap().bandwidth_kib;
        if task == 0 {
            settings.bandwidth_kib
        } else if settings.bandwidth_kib == 0 {
            task
        } else {
            task.min(settings.bandwidth_kib)
        }
    }
    pub(crate) fn resumed(&self, bytes: u64) {
        self.confirmed_bytes.store(bytes, Ordering::Release);
        self.update(|s| {
            s.resumed_bytes = bytes;
            s.transferred_bytes = bytes;
        });
    }
    pub(crate) fn digest(&self, digest: String) {
        self.update(|s| s.sha256 = Some(digest));
    }
}

struct RunningGuard {
    state: Weak<AppState>,
    id: String,
}
impl Drop for RunningGuard {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            state.transfers.running.lock().unwrap().remove(&self.id);
        }
    }
}

impl TransferManager {
    pub fn pause_all(&self) {
        for h in self.items.lock().unwrap().values() {
            if h.control
                .compare_exchange(0, 4, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                h.update(|s| {
                    s.status = "paused".into();
                    s.phase = "paused".into();
                });
                let _ = h.persist();
            }
        }
    }
    pub fn restore(&self, state: &Arc<AppState>) -> Result<()> {
        *self.state.lock().unwrap() = Arc::downgrade(state);
        let records = state.database.lock().unwrap().transfers()?;
        let mut items = self.items.lock().unwrap();
        for record in records {
            let mut view: Snapshot = serde_json::from_value(record["snapshot"].clone())?;
            let mut op: Operation = serde_json::from_value(record["operation"].clone())?;
            op.transfer_request =
                serde_json::from_value(record["operation"]["transfer_request"].clone())?;
            let checkpoint = serde_json::from_value(record["checkpoint"].clone())?;
            let server: Option<Server> = serde_json::from_value(record["server"].clone())?;
            if !terminal(&view.status) {
                view.status = "paused".into();
                view.phase = "paused".into();
                view.next_retry_at = None;
                view.bytes_per_second = 0;
                view.version += 1;
            }
            let confirmed = view.transferred_bytes;
            let destination_key = destination_key(server.as_ref(), &op);
            let id = view.transfer_id.clone();
            let h = Arc::new(Handle {
                snapshot: Mutex::new(view),
                notify: tokio::sync::Notify::new(),
                cancel: CancellationToken::new(),
                control: AtomicU8::new(3),
                confirmed_bytes: AtomicU64::new(confirmed),
                destination_key,
                created: Instant::now(),
                finished: Mutex::new(None),
                state: Mutex::new(Arc::downgrade(state)),
                operation: Mutex::new(op),
                origin: Mutex::new(record["origin"].as_str().unwrap_or("mcp").into()),
                checkpoint: Mutex::new(checkpoint),
                worker_active: std::sync::atomic::AtomicBool::new(false),
                server_identity: Mutex::new(server),
                journal_lock: Mutex::new(()),
                lifecycle: tokio::sync::Mutex::new(()),
            });
            items.insert(id, h);
        }
        Ok(())
    }

    async fn acquire(&self, state: &Arc<AppState>, h: &Arc<Handle>) -> Result<RunningGuard> {
        let id = h.snapshot().transfer_id;
        loop {
            h.check_cancelled()?;
            let settings = state.settings.lock().unwrap().transfers.clone();
            {
                let items = self.items.lock().unwrap();
                let mut running = self.running.lock().unwrap();
                let endpoint = |item: &Handle| {
                    item.server_identity
                        .lock()
                        .unwrap()
                        .as_ref()
                        .map(|s| (s.host.clone(), s.port, s.username.clone()))
                };
                let own_endpoint = endpoint(h);
                let used = running
                    .iter()
                    .filter(|id| {
                        items
                            .get(*id)
                            .is_some_and(|item| endpoint(item) == own_endpoint)
                    })
                    .count();
                let own = h.snapshot();
                let earlier = running.len() < settings.max_concurrent
                    && used < settings.per_server_concurrent
                    && items.values().any(|item| {
                        let v = item.snapshot();
                        !running.contains(&v.transfer_id)
                            && item.worker_active.load(Ordering::Acquire)
                            && item.control.load(Ordering::Acquire) == 0
                            && v.status == "queued"
                            && (v.priority > own.priority
                                || (v.priority == own.priority && v.created_at < own.created_at))
                            && (endpoint(item) == own_endpoint
                                || running.len() + 1 >= settings.max_concurrent)
                            && running
                                .iter()
                                .filter(|id| {
                                    items
                                        .get(*id)
                                        .is_some_and(|active| endpoint(active) == endpoint(item))
                                })
                                .count()
                                < settings.per_server_concurrent
                    });
                if running.len() < settings.max_concurrent
                    && used < settings.per_server_concurrent
                    && !earlier
                {
                    running.insert(id.clone());
                    return Ok(RunningGuard {
                        state: Arc::downgrade(state),
                        id,
                    });
                }
                drop(running);
                drop(items);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn control_task(
        &self,
        state: &Arc<AppState>,
        op: &Operation,
        source: &str,
        h: &Arc<Handle>,
    ) -> Result<Value> {
        let _action = h.lifecycle.lock().await;
        anyhow::ensure!(
            self.items
                .lock()
                .unwrap()
                .get(&op.transfer_id)
                .is_some_and(|current| Arc::ptr_eq(current, h)),
            "TRANSFER_NOT_FOUND: Transfer was removed"
        );
        match op.transfer_action.as_str() {
            "pause" => {
                let accepted = h
                    .control
                    .compare_exchange(0, 4, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok();
                if accepted {
                    h.phase("pausing");
                }
                let mut value = serde_json::to_value(h.snapshot())?;
                value["pause_requested"] = json!(accepted);
                Ok(value)
            }
            "resume" | "retry" => {
                anyhow::ensure!(
                    !h.worker_active.load(Ordering::Acquire),
                    "Transfer worker is still stopping"
                );
                let status = h.snapshot().status;
                anyhow::ensure!(
                    matches!(status.as_str(), "paused" | "failed"),
                    "Only paused or failed transfers can be resumed"
                );
                let original = h.operation.lock().unwrap().clone();
                let server = state.server(&original.server_id)?;
                authorize(&server, &original, source)?;
                // A desktop picker grant must never be inherited by an MCP caller.
                if source == "mcp" {
                    *h.origin.lock().unwrap() = "mcp".into();
                }
                anyhow::ensure!(
                    h.worker_active
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok(),
                    "Transfer already resumed"
                );
                h.control.store(0, Ordering::Release);
                h.update(|s| {
                    s.status = "queued".into();
                    s.phase = "queued".into();
                    s.error = None;
                    s.next_retry_at = None;
                    s.finished_at = None;
                    s.retry_count = 0;
                });
                *h.finished.lock().unwrap() = None;
                if let Err(e) = h.persist() {
                    h.worker_active.store(false, Ordering::Release);
                    h.complete(&Err(error("JOURNAL_FAILED", "Cannot save transfer task")));
                    return Err(e);
                }
                spawn_worker(state.clone(), h.clone());
                Ok(serde_json::to_value(h.snapshot())?)
            }
            "configure" => {
                if let Some(priority) = op.priority {
                    anyhow::ensure!((-10..=10).contains(&priority), "Priority must be -10 to 10");
                }
                if let Some(rate) = op.bandwidth_kib {
                    anyhow::ensure!(rate <= 1_048_576, "Bandwidth limit is too large");
                }
                h.update(|s| {
                    if let Some(p) = op.priority {
                        s.priority = p;
                    }
                    if let Some(rate) = op.bandwidth_kib {
                        s.bandwidth_kib = rate;
                    }
                });
                h.persist()?;
                Ok(serde_json::to_value(h.snapshot())?)
            }
            "cleanup" => {
                anyhow::ensure!(
                    !h.worker_active.load(Ordering::Acquire)
                        && (terminal(&h.snapshot().status) || h.snapshot().status == "paused"),
                    "Stop the transfer before cleanup"
                );
                crate::transfer_io::cleanup(state, h, source).await?;
                let id = h.snapshot().transfer_id;
                state.database.lock().unwrap().delete_transfer(&id)?;
                self.items.lock().unwrap().remove(&id);
                Ok(json!({"transfer_id":id,"removed":true}))
            }
            _ => unreachable!(),
        }
    }
}

fn destination_key(server: Option<&Server>, op: &Operation) -> String {
    if op.kind == "upload_file" {
        let identity = server
            .map(|s| format!("{}:{}:{}", s.host, s.port, s.username))
            .unwrap_or_else(|| op.server_id.clone());
        format!(
            "remote:{}:{}",
            identity,
            op.transfer_request
                .as_ref()
                .map(|s| s.remote_path.as_str())
                .unwrap_or(&op.path)
        )
    } else {
        format!("local:{}", op.target)
    }
}

fn spawn_worker(state: Arc<AppState>, h: Arc<Handle>) {
    tokio::spawn(async move {
        let op = h.operation.lock().unwrap().clone();
        let source = if *h.origin.lock().unwrap() == "mcp" {
            "mcp"
        } else {
            "desktop"
        };
        let result = run_task(state.clone(), h.clone(), source).await;
        h.complete(&result);
        h.worker_active.store(false, Ordering::Release);
        if let Ok(server) = state.server(&op.server_id) {
            let value = serde_json::to_value(h.snapshot()).unwrap_or_default();
            if let Err(e) = state.log(
                &server,
                source,
                &op,
                &Ok(value),
                h.created.elapsed().as_millis() as u64,
            ) {
                h.update(|s| s.audit_error = Some(e.to_string()));
            }
        }
    });
}

async fn run_task(state: Arc<AppState>, h: Arc<Handle>, source: &'static str) -> Result<Value> {
    let _slot = state.transfers.acquire(&state, &h).await?;
    let mut op = h.operation.lock().unwrap().clone();
    let server = state.server(&op.server_id)?;
    let settings = state.settings.lock().unwrap().clone();
    authorize(&server, &op, source)?;
    crate::security::check(&settings, &op, source)?;
    let identity = h.server_identity.lock().unwrap().clone();
    anyhow::ensure!(
        identity
            .as_ref()
            .is_some_and(|s| serde_json::to_value(s).ok() == serde_json::to_value(&server).ok()),
        "Server configuration changed; create a new transfer"
    );
    let resolved = local_path(
        op.transfer_request
            .as_ref()
            .context("Missing transfer spec")?,
        op.kind == "upload_file",
        &settings,
        source,
    )?;
    anyhow::ensure!(
        resolved.to_string_lossy() == op.target,
        "Local path changed; create a new transfer"
    );
    let approved = crate::security::needs_approval(&settings, &op, source);
    if approved {
        if op.kind == "upload_file" && !op.transfer_request.as_ref().is_some_and(|s| s.directory) {
            h.phase("hashing");
            let path = resolved.clone();
            let hashing = h.clone();
            let digest = tokio::task::spawn_blocking(move || {
                crate::transfer_io::hash_local(&path, &hashing)
            })
            .await??;
            op.transfer_request.as_mut().unwrap().sha256 = Some(digest);
        }
        h.phase("awaiting_approval");
        let wait =
            state
                .approvals
                .wait_with_timeout(&server, &op, source, settings.approval_timeout_secs);
        tokio::pin!(wait);
        loop {
            tokio::select! {r=&mut wait=>{r?;break;},_=tokio::time::sleep(Duration::from_millis(100))=>h.check_cancelled()?}
        }
    }
    *h.operation.lock().unwrap() = op.clone();
    h.persist()?;
    let mut attempt = 0;
    loop {
        h.check_cancelled()?;
        let work_state = state.clone();
        let work_h = h.clone();
        let work_op = op.clone();
        let work_server = server.clone();
        let path = resolved.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<Value> {
            let live = work_state.server(&work_server.id)?;
            authorize(&live, &work_op, source)?;
            anyhow::ensure!(
                serde_json::to_value(&live)? == serde_json::to_value(&work_server)?,
                "Server configuration changed; create a new transfer"
            );
            let settings = work_state.settings.lock().unwrap().clone();
            crate::security::check(&settings, &work_op, source)?;
            anyhow::ensure!(
                approved || !crate::security::needs_approval(&settings, &work_op, source),
                "Approval policy changed; resume the transfer"
            );
            local_path(
                work_op.transfer_request.as_ref().unwrap(),
                work_op.kind == "upload_file",
                &settings,
                source,
            )?;
            work_h.update(|s| {
                s.status = "running".into();
                s.phase = "connecting".into();
                s.next_retry_at = None;
            });
            crate::transfer_io::transfer(
                &work_state,
                &live,
                &work_op,
                &settings,
                source,
                &path,
                &work_h,
            )
        })
        .await?;
        match result {
            Ok(value) => return Ok(value),
            Err(e) => {
                let current = state.settings.lock().unwrap().transfers.clone();
                if !crate::operation_error::retryable(&e)
                    || attempt >= current.max_retries
                    || h.control.load(Ordering::Acquire) != 0
                {
                    return Err(e);
                }
                attempt += 1;
                let jitter = (uuid::Uuid::new_v4().as_u128() % 500) as u64;
                let delay = Duration::from_millis(
                    (current.retry_base_secs * 1000 * (1u64 << (attempt - 1))).min(30000) + jitter,
                );
                h.update(|s| {
                    s.status = "retrying".into();
                    s.phase = "retrying".into();
                    s.retry_count = attempt;
                    s.error = Some(crate::operation_error::error_response(&e, &op));
                    s.next_retry_at = Some(
                        (chrono::Utc::now()
                            + chrono::Duration::milliseconds(delay.as_millis() as i64))
                        .to_rfc3339(),
                    );
                    s.bytes_per_second = 0;
                });
                h.persist()?;
                let until = Instant::now() + delay;
                while Instant::now() < until {
                    h.check_cancelled()?;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
}
