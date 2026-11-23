
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
        let now = Instant::now();
        let previous = {
            let mut current = self.phase_started.lock().unwrap();
            let previous = current.take();
            if phase != "queued" {
                *current = Some((phase.to_string(), now));
            }
            previous
        };
        if let Some((name, started)) = previous {
            let elapsed = now.duration_since(started).as_millis() as u64;
            match name.as_str() {
                "transferring" => self.update(|s| {
                    s.transfer_ms = s.transfer_ms.saturating_add(elapsed);
                }),
                "verifying" => self.update(|s| {
                    s.verify_ms = s.verify_ms.saturating_add(elapsed);
                }),
                _ => {}
            }
        }
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
            s.verified_bytes = bytes;
        });
    }
    pub(crate) fn digest(&self, digest: String) {
        self.update(|s| s.sha256 = Some(digest));
    }
}

impl TransferManager {
    pub(crate) fn configure(&self, settings: &TransferSettings) {
        self.scheduler.configure(settings);
    }
    pub fn pause_all(&self) {
        for h in self.items.lock().unwrap().values() {
            if h.control
                .compare_exchange(0, 4, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.scheduler.cancel(&h.snapshot().transfer_id);
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
        self.scheduler
            .configure(&state.settings.lock().unwrap().transfers);
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
                window: Mutex::new(Default::default()),
                last_progress: Mutex::new(Instant::now()),
                phase_started: Mutex::new(None),
                queued_recorded: std::sync::atomic::AtomicBool::new(false),
            });
            items.insert(id, h);
        }
        Ok(())
    }

    pub(crate) async fn acquire(
        &self,
        state: &Arc<AppState>,
        h: &Arc<Handle>,
    ) -> Result<RunPermit> {
        let settings = state.settings.lock().unwrap().transfers.clone();
        self.scheduler.configure(&settings);
        let endpoint = h
            .server_identity
            .lock()
            .unwrap()
            .as_ref()
            .map(|server| format!("{}:{}:{}", server.host, server.port, server.username))
            .unwrap_or_else(|| h.snapshot().server_id);
        let view = h.snapshot();
        self.scheduler
            .acquire(&view.transfer_id, &endpoint, view.priority, h)
            .await
    }

    pub(crate) fn pause_handle(&self, h: &Arc<Handle>) -> bool {
        let accepted = h
            .control
            .compare_exchange(0, 4, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if accepted {
            self.scheduler.cancel(&h.snapshot().transfer_id);
            h.phase("pausing");
        }
        accepted
    }

    pub(crate) async fn resume_handle(
        &self,
        state: &Arc<AppState>,
        h: &Arc<Handle>,
        source: &str,
    ) -> Result<()> {
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
        Ok(())
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
                let accepted = self.pause_handle(h);
                let mut value = serde_json::to_value(h.snapshot())?;
                value["pause_requested"] = json!(accepted);
                Ok(value)
            }
            "resume" => {
                self.resume_handle(state, h, source).await?;
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
                if let Some(priority) = op.priority {
                    self.scheduler.update_priority(&op.transfer_id, priority);
                }
                h.persist()?;
                Ok(serde_json::to_value(h.snapshot())?)
            }
            "cleanup" => {
                anyhow::ensure!(
                    !h.worker_active.load(Ordering::Acquire)
                        && (terminal(&h.snapshot().status) || h.snapshot().status == "paused"),
                    "Stop the transfer before cleanup"
                );
                crate::transfer_engine::cleanup(state, h, source).await?;
                let id = h.snapshot().transfer_id;
                self.scheduler.cancel(&id);
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
        if let Some(group) = h.snapshot().group_id {
            state
                .transfers
                .group_terminal(&group, &h.snapshot().status);
        }
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
    let op = h.operation.lock().unwrap().clone();
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
    *h.operation.lock().unwrap() = op.clone();
    h.persist()?;
    let mut attempt = 0;
    loop {
        h.check_cancelled()?;
        let permit = state.transfers.acquire(&state, &h).await?;
        h.check_cancelled()?;
        h.mark_running();
        let result = attempt_transfer(
            state.clone(),
            h.clone(),
            op.clone(),
            server.clone(),
            resolved.clone(),
            source,
        )
        .await;
        drop(permit);
        match result {
            Ok(value) => return Ok(value),
            Err(error) => {
                let current = state.settings.lock().unwrap().transfers.clone();
                if !crate::operation_error::retryable(&error)
                    || attempt >= current.max_retries
                    || h.control.load(Ordering::Acquire) != 0
                {
                    h.failure(&error);
                    return Err(error);
                }
                attempt += 1;
                if matches!(
                    failure_category(crate::operation_error::error_code(&error)),
                    "network" | "connection"
                ) {
                    h.window.lock().unwrap().failure(true);
                }
                let jitter = (uuid::Uuid::new_v4().as_u128() % 500) as u64;
                let delay = Duration::from_millis(
                    (current.retry_base_secs * 1000 * (1u64 << (attempt - 1))).min(30000) + jitter,
                );
                h.failure(&error);
                h.update(|s| {
                    s.status = "retrying".into();
                    s.phase = "retry_wait".into();
                    s.retry_count = attempt;
                    s.error = Some(crate::transfers::transfer_error_response(&error, &op));
                    s.next_retry_at = Some(
                        (chrono::Utc::now()
                            + chrono::Duration::milliseconds(delay.as_millis() as i64))
                        .to_rfc3339(),
                    );
                    s.bytes_per_second = 0;
                });
                h.retry_wait(delay);
                h.persist()?;
                wait_retry(&h, delay).await?;
                h.update(|s| {
                    s.status = "queued".into();
                    s.phase = "queued".into();
                    s.next_retry_at = None;
                });
            }
        }
    }
}

async fn attempt_transfer(
    state: Arc<AppState>,
    h: Arc<Handle>,
    op: Operation,
    server: Server,
    resolved: std::path::PathBuf,
    source: &'static str,
) -> Result<Value> {
    let idle = state
        .settings
        .lock()
        .unwrap()
        .transfers
        .idle_timeout_secs;
    let watchdog_handle = h.clone();
    let watchdog = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if watchdog_handle.control.load(Ordering::Acquire) != 0 {
                break;
            }
            if watchdog_handle
                .last_progress
                .lock()
                .unwrap()
                .elapsed()
                > Duration::from_secs(idle)
            {
                watchdog_handle.phase("stalled");
            }
        }
    });
    let joined = tokio::task::spawn_blocking(move || -> Result<Value> {
        let live = state.server(&server.id)?;
        authorize(&live, &op, source)?;
        anyhow::ensure!(
            serde_json::to_value(&live)? == serde_json::to_value(&server)?,
            "Server configuration changed; create a new transfer"
        );
        let settings = state.settings.lock().unwrap().clone();
        crate::security::check(&settings, &op, source)?;
        local_path(
            op.transfer_request.as_ref().unwrap(),
            op.kind == "upload_file",
            &settings,
            source,
        )?;
        h.update(|s| {
            s.status = "running".into();
            s.next_retry_at = None;
        });
        h.phase("connecting");
        crate::transfer_engine::transfer(&state, &live, &op, &settings, source, &resolved, &h)
    })
    .await;
    watchdog.abort();
    joined?
}

async fn wait_retry(h: &Handle, delay: Duration) -> Result<()> {
    let until = Instant::now() + delay;
    loop {
        h.check_cancelled()?;
        let notified = h.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let remaining = until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        tokio::select! {
            _ = tokio::time::sleep(remaining) => {},
            _ = notified => {},
        }
    }
}
