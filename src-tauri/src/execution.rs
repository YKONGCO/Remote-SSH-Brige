use crate::{
    jobs::{Job, Progress},
    model::{Operation, RuntimeSettings, Server},
    operation_error::error_with_details,
    security::ApprovalView,
    state::AppState,
};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{
    io::Read,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

fn acquire_channel(
    state: &Arc<AppState>,
    server: &Server,
    settings: &RuntimeSettings,
) -> Result<(crate::ssh_session::SessionLease, ssh2::Channel)> {
    let mut attempts = 0;
    loop {
        let mut lease = state.sessions.acquire(server, settings)?;
        match lease.session().channel_session() {
            Ok(channel) => return Ok((lease, channel)),
            Err(error) => {
                let reason = error.to_string();
                lease.release(false);
                attempts += 1;
                if attempts >= 2 {
                    return Err(error_with_details(
                        "SSH_CHANNEL_FAILED",
                        "Cannot open an SSH channel; the command was not sent",
                        json!({"command_sent":false,"outcome_unknown":false,"attempts":attempts,"reason":reason}),
                    ));
                }
            }
        }
    }
}

pub fn execute(
    state: &Arc<AppState>,
    server: &Server,
    op: &Operation,
    settings: &RuntimeSettings,
    source: &'static str,
    progress: &Progress,
    approval: Option<&ApprovalView>,
) -> Result<Value> {
    anyhow::ensure!(!op.command.trim().is_empty(), "command must not be empty");
    let (mut lease, mut channel) = acquire_channel(state, server, settings)?;
    let reused = lease.reused();
    let handoff = lease.handle();
    channel
        .exec(&crate::ssh::command_with_login_path(server, &op.command))
        .map_err(|error| {
            let message = error.to_string();
            error_with_details(
                "SSH_SUBMIT_FAILED",
                "Command submission failed; it was not retried and the remote outcome is unknown",
                json!({"command_sent":false,"outcome_unknown":true,"reason":message}),
            )
        })?;
    lease.session().set_blocking(false);
    let connection = {
        let mut value = json!({
            "reused": reused,
            "command_sent": true,
            "session": if reused { "reused" } else { "created" },
        });
        if let Some(approval) = approval {
            value["approval_id"] = json!(approval.id);
        }
        value
    };
    let mut execution = Job::new(&server.id);
    execution.server_name = server.name.clone();
    execution.command = op.command.clone();
    execution.source = source.into();
    let job = Arc::new(execution);
    job.set_connection(connection);

    struct Foreground(Arc<Job>);
    impl Drop for Foreground {
        fn drop(&mut self) {
            let mut data = self.0.data.lock().unwrap();
            data.decided = true;
            if !data.retained {
                data.close = true;
            }
            self.0.changed.notify_all();
        }
    }
    let _foreground = Foreground(job.clone());
    let worker_job = job.clone();
    let weak = Arc::downgrade(state);
    let worker_server = server.clone();
    let worker_op = op.clone();
    std::thread::Builder::new()
        .name("ssh-exec".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || -> Result<Option<i32>> {
                    let mut bytes = [0; 8192];
                    loop {
                        if worker_job.data.lock().unwrap().close {
                            let _ = channel.close();
                            let _ =
                                lease
                                    .session()
                                    .disconnect(None, "Execution session closed", None);
                            return Ok(None);
                        }

                        for stream_id in [0, 1] {
                            let mut stream = channel.stream(stream_id);
                            for _ in 0..16 {
                                match stream.read(&mut bytes) {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        let mut data = worker_job.data.lock().unwrap();
                                        if stream_id == 0 {
                                            data.stdout.append(&bytes[..n]);
                                        } else {
                                            data.stderr.append(&bytes[..n]);
                                        }
                                        worker_job.changed.notify_all();
                                    }
                                    Err(error)
                                        if error.kind() == std::io::ErrorKind::WouldBlock =>
                                    {
                                        break
                                    }
                                    Err(error) => return Err(error.into()),
                                }
                            }
                        }
                        if channel.eof() {
                            match channel.wait_close() {
                                Ok(()) => {
                                    if channel.exit_signal()?.exit_signal.is_some() {
                                        anyhow::bail!(
                                            "SSH command ended by signal; exit code unknown"
                                        );
                                    }
                                    return Ok(Some(channel.exit_status()?));
                                }
                                Err(error) if error.code() == ssh2::ErrorCode::Session(-37) => {}
                                Err(error) => return Err(error.into()),
                            }
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                },
            ));
            drop(channel);
            let (healthy, status, code, error) = match outcome {
                Ok(Ok(Some(code))) => (true, "exited", Some(code), None),
                Ok(Ok(None)) => (false, "closed", None, None),
                Ok(Err(error)) => (
                    false,
                    "disconnected",
                    None,
                    Some(crate::operation_error::error_message(&error)),
                ),
                Err(_) => (
                    false,
                    "disconnected",
                    None,
                    Some("SSH worker stopped unexpectedly".into()),
                ),
            };
            worker_job.finish(status, code, error);

            let mut data = worker_job.data.lock().unwrap();
            while !data.decided {
                data = worker_job.changed.wait(data).unwrap();
            }
            let retained = data.retained;
            drop(data);
            lease.release(healthy);
            if retained {
                audit_final(weak, &worker_server, &worker_op, source, &worker_job);
            }
        })
        .context("Cannot start SSH worker")?;
    let timeout = Duration::from_secs(if op.timeout_secs == 0 {
        settings.command_timeout_secs
    } else {
        op.timeout_secs.clamp(1, 300)
    });
    let deadline = Instant::now() + timeout;
    let (mut out, mut err) = (0, 0);
    let mut last_progress = Instant::now() - Duration::from_millis(100);
    loop {
        if last_progress.elapsed() >= Duration::from_millis(100) {
            (out, err) = crate::jobs::send_progress(progress, &job, out, err);
            last_progress = Instant::now();
        }
        let mut data = job.data.lock().unwrap();
        if data.status != "running" {
            data.close = true;
            job.changed.notify_all();
            drop(data);
            let mut value = job.snapshot(0, 0, crate::jobs::CAPACITY * 2)?;
            if value["status"] == "exited" {
                value.as_object_mut().unwrap().remove("job_id");
                value["command_sent"] = json!(true);
                value["outcome_unknown"] = json!(false);
                return Ok(value);
            }
            return Err(error_with_details(
                "SSH_DISCONNECTED",
                "SSH disconnected; final exit status is unknown",
                json!({
                    "command_sent": true,
                    "outcome_unknown": true,
                    "result": value,
                }),
            ));
        }
        if Instant::now() >= deadline {
            drop(data);
            let failure = if !settings.exec_timeout_to_job {
                Some((
                    "EXEC_TIMEOUT",
                    "Command timed out; job retention is disabled",
                ))
            } else if !state.server(&op.server_id)?.permits_mcp("job") {
                Some((
                    "JOB_PERMISSION_DENIED",
                    "Command timed out; missing job permission prevents retention",
                ))
            } else {
                state.jobs.retain(&job).err().map(|_| {
                    (
                        "JOB_LIMIT",
                        "Cannot retain session: server already has 4 active jobs",
                    )
                })
            };
            if let Some((code, reason)) = failure {
                let close_confirmed = job.close();
                let mut value = job.snapshot(0, 0, crate::jobs::CAPACITY * 2)?;
                value["close_confirmed"] = json!(close_confirmed);
                value["command_sent"] = json!(true);
                value["outcome_unknown"] = json!(true);
                return Err(error_with_details(code, reason, value));
            }

            handoff.promote_to_job();
            let status = job.data.lock().unwrap().status;
            let value = json!({
                "job_id": job.id,
                "server_id": server.id,
                "status": status,
                "command_sent": true,
                "outcome_unknown": false,
            });
            if let Err(error) = state.log(
                server,
                source,
                op,
                &Ok(value.clone()),
                timeout.as_millis() as u64,
            ) {
                eprintln!("Job handoff audit failed for {}: {error:#}", job.id);
            }
            job.changed.notify_all();
            return Ok(value);
        }
        let _guard = job
            .changed
            .wait_timeout(data, Duration::from_millis(100))
            .unwrap();
    }
}

fn audit_final(state: Weak<AppState>, server: &Server, op: &Operation, source: &str, job: &Job) {
    if let Some(state) = state.upgrade() {
        let mut op = op.clone();
        op.kind = "job".into();
        op.job_action = "finish".into();
        op.job_id = job.id.clone();
        let result = job.snapshot(0, 0, 16384);
        if let Err(error) = state.log_event(
            server,
            source,
            &op,
            &result,
            job.started.elapsed().as_millis() as u64,
            crate::model::AuditEventKind::JobFinished,
        ) {
            eprintln!("Job final audit failed: {error:#}");
        }
    }
}
