//! Durable failure-to-dispatch transition, shared by local and courier events.
use anyhow::{Context, Result, bail};
use serde_json::json;

use crate::contracts::Event;
use crate::paths::Ctx;
use crate::{events, launch, project::Project, thread, threads};

pub fn consume(ctx: &Ctx, project: &Project, event: &Event) -> Result<()> {
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
    let task = std::fs::read_to_string(thread::task_path(project, &record.id))
        .context("escalation_brief_missing")?;
    let state = launch::repository_state(
        ctx,
        (!record.repo.is_empty()).then_some(record.repo.as_str()),
        Some(&record.base),
    )?;
    let selected = launch::resolve_launch(
        ctx,
        project,
        &launch::ResolveInput {
            task: &task,
            state,
            workflow: &record.role,
            previous: Some(&record.launch),
            failure: Some(&failure.text),
        },
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
            // A refusal is durable and not retried every tick. In particular,
            // bound/exclusion/key errors cannot spin or silently reuse a model.
            launch::ledger(
                project,
                json!({"kind":"escalation-refused", "event":event.id,
                "thread":record.id, "failure":failure.text, "error":format!("{e:#}")}),
            )?;
            thread::update_checked(project, &record.id, |t| {
                if t.attempt != record.attempt || t.status == thread::Status::Resolved {
                    bail!("escalation_stale: lane changed during assessment");
                }
                t.failure_event = event.id.clone();
                t.last_failure = failure.text.clone();
                t.error = format!("{e:#}");
                t.status = thread::Status::Failed;
                t.prompt_pending = false;
                Ok(())
            })?;
            return Err(e);
        }
    }
    events::append_delivery(project, &event.id, crate::contracts::DeliveryState::Handled)?;
    Ok(())
}

pub fn tick(ctx: &Ctx, project: &Project) -> Result<()> {
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
