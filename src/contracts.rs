//! Shared ADE record types. Every field is named in SPEC-ADE; none is added.

use serde::{Deserialize, Serialize};

/// One `[recipes.<id>]` row: the full D2 row plus `provider` (reserved for the
/// pi move, question 25), `enabled`, `plain` and the per-lane `machine`
/// (SPEC-remote D2/D4, §4.1). An empty `machine` means the project default.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Recipe {
    pub kind: String,
    pub args: Vec<String>,
    pub env: Vec<String>,
    pub ready_timeout_ms: u64,
    pub provider: String,
    pub enabled: bool,
    pub plain: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub machine: String,
}

impl Default for Recipe {
    fn default() -> Self {
        Recipe {
            kind: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            ready_timeout_ms: 30_000,
            provider: String::new(),
            enabled: true,
            plain: String::new(),
            machine: String::new(),
        }
    }
}

/// The `local` machine sentinel: the Mac itself (SPEC-remote §4.1).
pub const MACHINE_LOCAL: &str = "local";

/// The box's PATH, passed to every box lane's `tab create --env`
/// (SPEC-remote §3.3, check c C4).
pub const BOX_PATH: &str =
    "/home/ubuntu/.local/bin:/home/ubuntu/.cargo/bin:/usr/local/bin:/usr/bin:/bin";
/// The plugin binary and ADE root on the box (SPEC-remote §4.2 step 6).
pub const BOX_BIN: &str = "/home/ubuntu/.local/bin/herdr-ade";
pub const BOX_ROOT: &str = "/home/ubuntu/.herdr-ade";
/// The box's per-lane build folders (SPEC-remote §3.2).
pub const BOX_BUILD: &str = "/home/ubuntu/build/lanes";

/// The fixed box command prefix `/home/ubuntu/.local/bin/herdr-ade --root
/// /home/ubuntu/.herdr-ade` (SPEC-remote D12/D14, §4.2 step 6).
pub fn box_prefix() -> String {
    format!("{BOX_BIN} --root {BOX_ROOT}")
}

/// The box path of one lane's card (SPEC-remote §4.3).
pub fn box_lane_card(slug: &str, thread: &str) -> String {
    format!("{BOX_ROOT}/{slug}/lanes/{thread}.toml")
}

/// A saved machine's stable profile (SPEC-remote §4.1). `id` is the plugin's
/// identity; `label` is renameable and is only shown.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineProfile {
    pub id: String,
    pub label: String,
    pub target: String,
    pub session: String,
}

impl MachineProfile {
    pub fn is_local(&self) -> bool {
        self.id == MACHINE_LOCAL || self.label == MACHINE_LOCAL
    }
}

/// One row of the Mac→box repository map (SPEC-remote §4.1). The plugin never
/// derives a box path from a Mac path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxRepoMap {
    pub mac: &'static str,
    pub box_path: &'static str,
    pub publish_url: &'static str,
}

/// The committed default map: the two tool repositories. Other projects add
/// their own row before first remote use.
pub const BOX_REPOS: &[BoxRepoMap] = &[
    BoxRepoMap {
        mac: "/Users/rolfie/projects/herdr",
        box_path: "/home/ubuntu/projects/herdr",
        publish_url: "https://github.com/uguryildirim24/herdr.git",
    },
    BoxRepoMap {
        mac: "/Users/rolfie/projects/herdr-ade",
        box_path: "/home/ubuntu/projects/herdr-ade",
        publish_url: "https://github.com/uguryildirim24/herdr-ade.git",
    },
];

/// The box's copy of one lane's start record (SPEC-remote §4.2 step 5, §4.3).
/// Written by the Mac after the box pane id exists; the box validates
/// `HERDR_ADE_LAUNCH`, `HERDR_PANE_ID`, cwd and process identity against it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct LaneCard {
    pub project: String,
    pub thread: String,
    pub attempt: u32,
    pub brief_hash: String,
    pub role: String,
    pub kind: String,
    pub pane_id: String,
    /// The renameable label the board shows.
    pub machine_label: String,
    /// The stable profile id the Mac supplied (SPEC-remote §4.1).
    pub machine_id: String,
    /// The box clone and checkout paths; never derived from the Mac path.
    pub box_repo: String,
    pub box_worktree: String,
    /// The brief commit `B` the box fetch verified as `FETCH_HEAD`.
    pub brief_commit: String,
    /// The lane branch and the URL-matched remote it publishes to
    /// (SPEC-remote §4.2 step 7). `ha done` checks the published ref.
    pub branch: String,
    pub publish_url: String,
    pub recipient: Recipient,
    /// The exact line typed at start (SPEC-remote §4.2 step 6).
    pub start_line: String,
    pub created: String,
}

/// The thread record's `launch` object: the chosen recipe's full D2 row.
/// `brief_hash` is filled after the brief commit; `attempt` is 1 at resolve
/// time (D2, D9).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Launch {
    pub kind: String,
    pub args: Vec<String>,
    pub env: Vec<String>,
    pub ready_timeout_ms: u64,
    pub policy_hash: String,
    pub attempt: u32,
    pub brief_hash: String,
    pub recipe_id: String,
    pub reason: String,
    /// The compact `<job> runs on <plain>` sentence for the board's
    /// `ade_last` token (D17 item 14), stored on the record so the ticker
    /// never rereads live config.
    pub compact_reason: String,
    /// The per-lane machine the roles-table row chose, empty for the project
    /// default (SPEC-remote D2/D4, §4.1).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub machine: String,
}

/// Process identity from `pane process-info` once the agent is ready
/// (SPEC-ADE D3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub argv0: String,
}

/// Identity binding compared on live reads. `terminal_id` is never stored
/// or compared (SPEC-ADE D3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct IdentityBinding {
    pub socket: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
    pub cwd: String,
    /// Absent on an adopted thread (SPEC-ADE D3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<String>,
}

/// A role's resolved row (SPEC-ADE D2): the `default` recipe of
/// `[roles.<name>]`, then the PROJECT.md override.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RoleSpec {
    pub kind: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub ready_timeout_ms: u64,
}

/// `done` or `waiting` (SPEC-ADE D5).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    Done,
    Waiting,
}

/// Complete requested payload stored at reserve so a later seal needs no
/// helper memory (SPEC-ADE D5, item 32).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Requested {
    Done { sha: String, report_path: String },
    Waiting { text: String },
}

/// Coordinator pane and attempt that must receive the sealed event
/// (SPEC-ADE D5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Recipient {
    pub pane: String,
    pub coordinator_attempt: u32,
}

/// Op state machine (SPEC-ADE D5).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpState {
    Reserved,
    Staged,
    Sealed,
    Abandoned,
}

/// `ops/<op id>.toml`. Op id is `<thread>-<attempt>-<n>`. The event id is
/// this op id, fixed at reserve (SPEC-ADE D5, item 32).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Op {
    pub op: String,
    pub revision: u32,
    pub thread: String,
    pub attempt: u32,
    pub kind: OpKind,
    pub recipient: Recipient,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<String>,
    pub helper_pid: u32,
    pub requested: Requested,
    /// Fixed event id: equal to `op` (SPEC-ADE D5, item 32).
    pub event: String,
    pub state: OpState,
    pub created: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
}

/// Sealed `done` payload (SPEC-ADE D5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DonePayload {
    pub sha: String,
    pub report_path: String,
    pub artifact: String,
}

/// Sealed `waiting` payload (SPEC-ADE D5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WaitingPayload {
    pub text: String,
}

/// Tagged event payload: `payload.done` or `payload.waiting` (SPEC-ADE D5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EventPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<DonePayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<WaitingPayload>,
}

/// Immutable sealed event `events/<event id>.toml` (SPEC-ADE D5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    pub id: String,
    pub op: String,
    pub thread: String,
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<String>,
    pub recipient: Recipient,
    pub created: String,
    pub payload: EventPayload,
}

/// Delivery journal states appended to `deliveries/<event id>.jsonl`
/// (SPEC-ADE D5).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Submitted,
    Acknowledged,
    Handled,
}

/// One line of the delivery journal (SPEC-ADE D5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeliveryLine {
    pub event: String,
    pub state: DeliveryState,
}

/// Durable `asks/<ask id>/r<revision>.toml` written before any publication
/// (SPEC-ADE D17 item 4).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Ask {
    pub id: String,
    pub revision: u32,
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<String>,
    pub question: String,
    pub choices: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub what: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub means: Option<String>,
    pub asked: String,
    pub coordinator_binding: String,
}

/// The only values `publish()` accepts (SPEC-ADE D17 item 3, item 35).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HumanMessage {
    Say {
        what: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        means: Option<String>,
    },
    Ask {
        id: String,
        revision: u32,
    },
    Notice {
        id: String,
    },
}

/// Completion pin projected onto a manifest member from a sealed `done`
/// event (SPEC-ADE D6, item 33).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CompletionPin {
    pub event: String,
    pub attempt: u32,
    pub sha: String,
    pub artifact: String,
}

/// One admitted lane in the round manifest (SPEC-ADE D6, item 33).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ManifestMember {
    pub thread: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<CompletionPin>,
}

/// Authoritative admitted set. Membership is never inferred from completions
/// (SPEC-ADE D6, item 33).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AdmissionManifest {
    pub revision: u64,
    #[serde(default)]
    pub members: Vec<ManifestMember>,
}

/// `.state/rounds/r<n>.toml` (SPEC-ADE D6, item 33).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RoundRecord {
    pub round: String,
    pub branch: String,
    pub plain: String,
    #[serde(default)]
    pub gates: Vec<String>,
    pub policy_hash: String,
    pub manifest: AdmissionManifest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
    /// When `ha round open` wrote the record; orders `GLOSSARY.md` (A3).
    #[serde(default)]
    pub opened: String,
    /// Repository the integration branch lives in, fixed at open (A3).
    #[serde(default)]
    pub repo: String,
    /// Manifest revision frozen at the review brief commit `B` (SPEC-ADE D6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_revision: Option<u64>,
    /// `review/r<n>`, created from `B` (SPEC-ADE D6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_branch: Option<String>,
    /// The reviewer thread whose sealed `done` sha is `V` (SPEC-ADE D6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
}

/// Checkpoint intent bound to `V` and the HANDOFF payload hash
/// (SPEC-ADE D6, item 34).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CheckpointIntent {
    pub parent: String,
    pub op: String,
    pub payload_hash: String,
}

/// Merge transaction phase (SPEC-ADE D6, item 34).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MergePhase {
    Intent,
    Merged,
    Checkpointed,
    MergeDiverged,
}

/// `.state/rounds/r<n>/merge.toml` (SPEC-ADE D6, item 34).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MergeIntent {
    pub op: String,
    pub expected_old: String,
    pub candidate: String,
    pub verdict: String,
    pub phase: MergePhase,
    /// The commit the integration branch held after merging V in: `V` on a
    /// fast-forward, otherwise a merge commit whose first parent is the moved
    /// head (SPEC-ADE D6, item 34). The checkpoint commits on top of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<CheckpointIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
}

/// Talk inbound request states (SPEC-ADE D18 item 2, item 35).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TalkRequestState {
    Queued,
    Submitted,
    Uncertain,
    Accepted,
}

/// `inbound { request, state }` on the talk journal (SPEC-ADE D18 item 2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalkInbound {
    pub request: String,
    pub state: TalkRequestState,
    pub recipient: Recipient,
}

/// One JSON object on `talk/journal.jsonl` (SPEC-ADE D18 items 2 and 6).
#[cfg(test)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalkJournalRecord {
    pub seq: u64,
    pub inbound: TalkInbound,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_roundtrip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, value, "{json}");
    }

    fn toml_roundtrip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let text = toml::to_string(value).unwrap();
        let back: T = toml::from_str(&text).unwrap();
        assert_eq!(&back, value, "{text}");
    }

    fn both<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        json_roundtrip(value);
        toml_roundtrip(value);
    }

    #[test]
    fn op_and_event_roundtrip() {
        let op = Op {
            op: "t-0001-1-1".into(),
            revision: 2,
            thread: "t-0001".into(),
            attempt: 1,
            kind: OpKind::Done,
            recipient: Recipient {
                pane: "w1:p1".into(),
                coordinator_attempt: 1,
            },
            round: Some("r1".into()),
            helper_pid: 4242,
            requested: Requested::Done {
                sha: "abc".into(),
                report_path: ".reports/a.md".into(),
            },
            event: "t-0001-1-1".into(),
            state: OpState::Staged,
            created: "2026-09-18T00:00:00Z".into(),
            artifact: Some("deadbeef".into()),
        };
        both(&op);
        both(&Event {
            id: op.op.clone(),
            op: op.op.clone(),
            thread: op.thread.clone(),
            attempt: op.attempt,
            round: op.round.clone(),
            recipient: op.recipient.clone(),
            created: op.created.clone(),
            payload: EventPayload {
                done: Some(DonePayload {
                    sha: "abc".into(),
                    report_path: ".reports/a.md".into(),
                    artifact: "deadbeef".into(),
                }),
                waiting: None,
            },
        });
        both(&Event {
            id: "t-0002-1-1".into(),
            op: "t-0002-1-1".into(),
            thread: "t-0002".into(),
            attempt: 1,
            round: None,
            recipient: Recipient {
                pane: "w1:p1".into(),
                coordinator_attempt: 1,
            },
            created: "2026-09-18T00:00:00Z".into(),
            payload: EventPayload {
                done: None,
                waiting: Some(WaitingPayload {
                    text: "need a look".into(),
                }),
            },
        });
    }

    #[test]
    fn delivery_line_roundtrip() {
        json_roundtrip(&DeliveryLine {
            event: "t-0001-1-1".into(),
            state: DeliveryState::Submitted,
        });
        json_roundtrip(&DeliveryLine {
            event: "t-0001-1-1".into(),
            state: DeliveryState::Acknowledged,
        });
        json_roundtrip(&DeliveryLine {
            event: "t-0001-1-1".into(),
            state: DeliveryState::Handled,
        });
    }

    #[test]
    fn ask_and_human_message_roundtrip() {
        both(&Ask {
            id: "a-1".into(),
            revision: 1,
            project: "demo".into(),
            round: Some("r1".into()),
            question: "keep the experiment running another hour?".into(),
            choices: vec!["keep it running another hour".into(), "stop it now".into()],
            what: Some("A lane is waiting.".into()),
            means: Some("You choose whether it continues.".into()),
            asked: "2026-09-18T00:00:00Z".into(),
            coordinator_binding: "w1:p1".into(),
        });
        json_roundtrip(&HumanMessage::Say {
            what: "A lane is done.".into(),
            means: None,
        });
        json_roundtrip(&HumanMessage::Ask {
            id: "a-1".into(),
            revision: 1,
        });
        json_roundtrip(&HumanMessage::Notice {
            id: "plain_exhausted".into(),
        });
    }

    #[test]
    fn recipe_and_launch_roundtrip() {
        let recipe = Recipe {
            kind: "cursor".into(),
            args: vec!["--model".into(), "cursor-grok-4.6-xhigh".into()],
            env: vec![],
            ready_timeout_ms: 30_000,
            provider: "cursor".into(),
            enabled: true,
            plain: "the usual coding helper".into(),
            machine: String::new(),
        };
        both(&recipe);
        both(&Launch {
            kind: "agy".into(),
            args: vec!["--model".into(), "gemini-3.8-flash-high".into()],
            env: vec![],
            ready_timeout_ms: 60_000,
            policy_hash: "cc".into(),
            attempt: 1,
            brief_hash: String::new(),
            recipe_id: "agy_gemini_flash".into(),
            reason: "this task runs on the web research helper, the usual choice.".into(),
            compact_reason: "this task runs on the web research helper".into(),
            machine: "oci".into(),
        });
    }

    #[test]
    fn round_merge_checkpoint_and_talk_roundtrip() {
        both(&RoundRecord {
            round: "r1".into(),
            branch: "main".into(),
            plain: "The first round lands the contracts.".into(),
            gates: vec!["cargo test --locked".into()],
            policy_hash: "cc".into(),
            manifest: AdmissionManifest {
                revision: 2,
                members: vec![ManifestMember {
                    thread: "t-0001".into(),
                    pin: Some(CompletionPin {
                        event: "t-0001-1-1".into(),
                        attempt: 1,
                        sha: "abc".into(),
                        artifact: "deadbeef".into(),
                    }),
                }],
            },
            expected_head: Some("bbb".into()),
            manifest_hash: Some("mh".into()),
            opened: "2026-09-18T00:00:00Z".into(),
            repo: "/repo".into(),
            frozen_revision: Some(2),
            review_branch: Some("review/r1".into()),
            reviewer: Some("t-0003".into()),
        });
        both(&MergeIntent {
            op: "merge-r1".into(),
            expected_old: "B".into(),
            candidate: "C".into(),
            verdict: "V".into(),
            merged: None,
            phase: MergePhase::Merged,
            checkpoint: Some(CheckpointIntent {
                parent: "V".into(),
                op: "merge-r1".into(),
                payload_hash: "hh".into(),
            }),
            head: None,
        });
        json_roundtrip(&TalkJournalRecord {
            seq: 3,
            inbound: TalkInbound {
                request: "req-1".into(),
                state: TalkRequestState::Queued,
                recipient: Recipient {
                    pane: "w1:p1".into(),
                    coordinator_attempt: 1,
                },
            },
        });
        json_roundtrip(&TalkJournalRecord {
            seq: 4,
            inbound: TalkInbound {
                request: "req-1".into(),
                state: TalkRequestState::Uncertain,
                recipient: Recipient {
                    pane: "w1:p1".into(),
                    coordinator_attempt: 1,
                },
            },
        });
        json_roundtrip(&TalkJournalRecord {
            seq: 5,
            inbound: TalkInbound {
                request: "req-1".into(),
                state: TalkRequestState::Submitted,
                recipient: Recipient {
                    pane: "w1:p1".into(),
                    coordinator_attempt: 1,
                },
            },
        });
        json_roundtrip(&TalkJournalRecord {
            seq: 6,
            inbound: TalkInbound {
                request: "req-1".into(),
                state: TalkRequestState::Accepted,
                recipient: Recipient {
                    pane: "w1:p1".into(),
                    coordinator_attempt: 1,
                },
            },
        });
    }
}
