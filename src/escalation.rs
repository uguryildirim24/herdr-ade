//! Durable failure-to-dispatch transition, shared by local and courier events.
use anyhow::{Context, Result, bail};
use serde_json::json;

use crate::contracts::{Event, FailureClass};
use crate::paths::Ctx;
use crate::{events, launch, project::Project, thread, threads};

pub(crate) fn consume(ctx: &Ctx, project: &Project, event: &Event) -> Result<()> {
    let Some(failure) = &event.payload.failed else {
        return Ok(());
    };
    if event.payload.done.is_some() || event.payload.waiting.is_some() {
        bail!("event_payload_invalid");
    }
    let record = thread::load(project, &event.thread)?;
    if crate::round::latest_event(&events::list(project), &record.id, event.attempt)
        .is_some_and(|latest| latest.id != event.id)
    {
        return Ok(());
    }
    if record.attempt.max(1) != event.attempt
        || record.failure_event == event.id
        || record.status == thread::Status::Resolved
    {
        return Ok(());
    }
    if record.kind == thread::Kind::Adopted {
        bail!("escalation_adopted: an adopted process cannot be replaced");
    }
    if failure.class == FailureClass::Unknown {
        thread::update(project, &record.id, |t| {
            t.failure_event = event.id.clone();
            t.last_failure = failure.text.clone();
            t.failure_class = FailureClass::Unknown;
            t.provider_failure_kind = None;
            t.status = thread::Status::Failed;
            t.error = format!(
                "WAITING: {}: {}",
                FailureClass::Unknown.plain(),
                failure.text
            );
            t.prompt_pending = false;
        })?;
        events::append_delivery(project, &event.id, crate::contracts::DeliveryState::Handled)?;
        return Ok(());
    }
    let task = std::fs::read_to_string(thread::task_path(project, &record.id))
        .context("escalation_brief_missing")?;
    // Provider outages and lost connections are infrastructure failures: retry
    // the exact recipe. A gone process restarts the attempt. Only failed work
    // consumes the routing table's retries and ordered fallbacks.
    let selected = launch::resolve_failure(
        ctx,
        project,
        &launch::ResolveInput {
            task: &task,
            workflow: &record.role,
            previous: Some(&record.launch),
            failure: Some(&failure.text),
            source_truncation: record.launch.source_truncation.as_ref(),
            ..Default::default()
        },
        failure.class,
    );
    match selected {
        Ok(mut selected) => {
            selected.attempt = record.attempt.max(1).saturating_add(1);
            selected.brief_hash = record.launch.brief_hash.clone();
            thread::update_checked(project, &record.id, |t| {
                if t.attempt != record.attempt
                    || t.pane_id != record.pane_id
                    || t.status == thread::Status::Resolved
                {
                    bail!("escalation_stale: lane changed during assessment");
                }
                t.failure_event = event.id.clone();
                t.last_failure = failure.text.clone();
                t.failure_class = failure.class;
                t.provider_failure_kind = failure.provider_kind.clone();
                t.attempt = selected.attempt;
                t.agent = selected.kind.clone();
                t.launch = selected;
                t.escalation_pending = true;
                t.status = thread::Status::Failed;
                t.prompt_pending = false;
                t.launch_attempts = 0;
                t.bootstrap.clear();
                t.error.clear();
                Ok(())
            })?;
        }
        Err(e) => {
            // A refusal is durable and is never retried every tick. Exhausted
            // recovery is normal: leave the failed lane in front of the
            // coordinator instead of selecting another recipe.
            let detail = format!("{e:#}");
            let exhausted = detail.starts_with("recovery_exhausted:");
            launch::ledger(
                project,
                json!({"kind": if exhausted { "recovery-exhausted" } else { "recovery-refused" },
                "event":event.id, "thread":record.id, "failure":failure.text, "error":detail}),
            )?;
            thread::update_checked(project, &record.id, |t| {
                if t.attempt != record.attempt || t.status == thread::Status::Resolved {
                    bail!("escalation_stale: lane changed during assessment");
                }
                t.failure_event = event.id.clone();
                t.last_failure = failure.text.clone();
                t.failure_class = failure.class;
                t.provider_failure_kind = failure.provider_kind.clone();
                t.error = if exhausted {
                    format!("WAITING: {detail}")
                } else {
                    detail.clone()
                };
                t.status = thread::Status::Failed;
                t.prompt_pending = false;
                Ok(())
            })?;
            if exhausted {
                events::append_delivery(
                    project,
                    &event.id,
                    crate::contracts::DeliveryState::Handled,
                )?;
                return Ok(());
            }
            return Err(e);
        }
    }
    events::append_delivery(project, &event.id, crate::contracts::DeliveryState::Handled)?;
    Ok(())
}

pub(crate) fn tick(ctx: &Ctx, project: &Project) -> Result<()> {
    let mut first = None;
    for event in events::list(project)
        .into_iter()
        .filter(|e| e.payload.failed.is_some())
    {
        if let Err(e) = consume(ctx, project, &event) {
            first.get_or_insert(e);
        }
    }
    for record in thread::list(project)
        .into_iter()
        .filter(|t| t.escalation_pending && t.status != thread::Status::Resolved)
    {
        if let Err(e) = threads::place_escalation(ctx, project, &record) {
            first.get_or_insert(e);
        }
    }
    first.map_or(Ok(()), Err)
}
