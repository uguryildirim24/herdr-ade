use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Write;

use serde::Serialize;

#[derive(Default)]
struct State {
    active: bool,
    json: bool,
    command: String,
    outcome: String,
    data: BTreeMap<String, serde_json::Value>,
    stdout: String,
    stderr: String,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

#[derive(Serialize)]
struct ResultRecord {
    outcome: String,
    command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    message: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    data: BTreeMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "String::is_empty")]
    warnings: String,
}

pub fn begin(
    json: bool,
    command: String,
    outcome: String,
    data: BTreeMap<String, serde_json::Value>,
) {
    STATE.with(|state| {
        *state.borrow_mut() = State {
            active: true,
            json,
            command,
            outcome,
            data,
            stdout: String::new(),
            stderr: String::new(),
        };
    });
}

pub fn structured() -> bool {
    STATE.with(|state| {
        let state = state.borrow();
        state.active && state.json
    })
}

pub fn set_outcome(outcome: impl Into<String>) {
    STATE.with(|state| state.borrow_mut().outcome = outcome.into());
}

pub fn insert(key: impl Into<String>, value: impl Into<serde_json::Value>) {
    STATE.with(|state| {
        state.borrow_mut().data.insert(key.into(), value.into());
    });
}

/// Complete one command from its typed facts. The ordinary text and warnings
/// are emitted here too, so JSON and human output are two renderings of the
/// same result rather than independently assembled answers.
pub fn success(
    outcome: Option<&str>,
    data: &impl Serialize,
    message: &str,
    warnings: &str,
) -> anyhow::Result<()> {
    let value = serde_json::to_value(data)?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("command result data must be an object"))?;
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if let Some(outcome) = outcome {
            state.outcome = outcome.to_string();
        }
        state.data.extend(
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    });
    write_stdout(format_args!("{message}"));
    write_stderr(format_args!("{warnings}"));
    Ok(())
}

pub fn write_stdout(args: std::fmt::Arguments<'_>) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.active && state.json {
            use std::fmt::Write as _;
            let _ = state.stdout.write_fmt(args);
        } else {
            let _ = std::io::stdout().lock().write_fmt(args);
        }
    });
}

pub fn write_stderr(args: std::fmt::Arguments<'_>) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.active && state.json {
            use std::fmt::Write as _;
            let _ = state.stderr.write_fmt(args);
        } else {
            let _ = std::io::stderr().lock().write_fmt(args);
        }
    });
}

fn take() -> State {
    STATE.with(|state| std::mem::take(&mut *state.borrow_mut()))
}

pub fn finish_success() -> std::io::Result<()> {
    render(take(), None)
}

pub fn finish_error(reason: &str) -> std::io::Result<()> {
    render(take(), Some(reason))
}

fn render(mut state: State, reason: Option<&str>) -> std::io::Result<()> {
    if !state.active {
        if let Some(reason) = reason {
            writeln!(std::io::stderr().lock(), "herdr-ade: {reason}")?;
        }
        return Ok(());
    }
    if state.json {
        let record = ResultRecord {
            outcome: if reason.is_some() {
                "refused".into()
            } else {
                state.outcome
            },
            command: state.command,
            reason: reason.map(str::to_string),
            message: state.stdout,
            data: state.data,
            warnings: state.stderr,
        };
        let mut stdout = std::io::stdout().lock();
        serde_json::to_writer_pretty(&mut stdout, &record)?;
        writeln!(stdout)
    } else {
        if let Some(reason) = reason {
            writeln!(std::io::stderr().lock(), "herdr-ade: {reason}")?;
        }
        state.active = false;
        Ok(())
    }
}
