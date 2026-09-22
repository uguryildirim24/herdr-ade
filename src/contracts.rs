//! Shared ADE record types. Every field is named in SPEC-ADE; none is added.

use serde::{Deserialize, Serialize};

fn is_false(value: &bool) -> bool {
    !*value
}

/// One executable `[recipes.<id>]` row. Selection lives in `[routing]` and
/// placement belongs to `[dispatch]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Recipe {
    pub(crate) kind: String,
    pub(crate) args: Vec<String>,
    pub(crate) env: Vec<String>,
    pub(crate) ready_timeout_ms: u64,
    pub(crate) provider: String,
    /// Runtime features this recipe can satisfy (for example `pictures`).
    pub(crate) capabilities: Vec<String>,
    pub(crate) enabled: bool,
    pub(crate) plain: String,
}

impl Default for Recipe {
    fn default() -> Self {
        Recipe {
            kind: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            ready_timeout_ms: 30_000,
            provider: String::new(),
            capabilities: Vec::new(),
            enabled: true,
            plain: String::new(),
        }
    }
}

/// The `local` machine sentinel: the Mac itself (SPEC-remote §4.1).
pub(crate) const MACHINE_LOCAL: &str = "local";

/// The local machine's display label, the one herdr's sidebar shows. It is the
/// machine part of a cross-machine `parent` token (the fork lane t-0053 form).
pub(crate) const MACHINE_LOCAL_LABEL: &str = "Local";

/// A saved machine's stable profile (SPEC-remote §4.1). `id` is the plugin's
/// identity; `label` is renameable and is only shown.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct MachineProfile {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) target: String,
    pub(crate) session: String,
}

impl MachineProfile {
    pub(crate) fn is_local(&self) -> bool {
        self.id == MACHINE_LOCAL || self.label == MACHINE_LOCAL
    }
}

/// The box's copy of one lane's start record (SPEC-remote §4.2 step 5, §4.3).
/// Written by the Mac after the box pane id exists; the box validates
/// `HERDR_ADE_LAUNCH`, `HERDR_PANE_ID`, cwd and process identity against it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub(crate) struct LaneCard {
    pub(crate) project: String,
    pub(crate) thread: String,
    pub(crate) attempt: u32,
    pub(crate) brief_hash: String,
    pub(crate) role: String,
    pub(crate) kind: String,
    pub(crate) pane_id: String,
    /// The renameable label the board shows.
    pub(crate) machine_label: String,
    /// The stable profile id the Mac supplied (SPEC-remote §4.1).
    pub(crate) machine_id: String,
    /// The box clone and checkout paths; never derived from the Mac path.
    pub(crate) box_repo: String,
    pub(crate) box_worktree: String,
    /// The brief commit `B` the box fetch verified as `FETCH_HEAD`.
    pub(crate) brief_commit: String,
    /// The lane branch and the URL-matched remote it publishes to
    /// (SPEC-remote §4.2 step 7). `ha done` checks the published ref.
    pub(crate) branch: String,
    pub(crate) publish_url: String,
    pub(crate) recipient: Recipient,
    /// The exact line typed at start (SPEC-remote §4.2 step 6).
    pub(crate) start_line: String,
    pub(crate) created: String,
}

/// The thread record's `launch` object: the chosen recipe's full D2 row.
/// `brief_hash` is filled after the brief commit; `attempt` is 1 at resolve
/// time (D2, D9).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub(crate) struct Launch {
    pub(crate) kind: String,
    pub(crate) args: Vec<String>,
    pub(crate) env: Vec<String>,
    pub(crate) ready_timeout_ms: u64,
    pub(crate) policy_hash: String,
    pub(crate) attempt: u32,
    pub(crate) brief_hash: String,
    /// The SHA-256 of the role's skill text at the moment this lane (or the
    /// coordinator) was primed. Staleness compares it with the skill file now
    /// on disk, so a running agent stuck on an old copy is visible (LEAN U4).
    #[serde(default)]
    pub(crate) skill_hash: String,
    pub(crate) recipe_id: String,
    /// Number of failed-work recovery selections after the first launch.
    /// Infrastructure retries never advance this fallback selector.
    pub(crate) escalations: u32,
    /// Number of bounded same-recipe retries for provider, connection, and
    /// process failures.
    #[serde(default)]
    pub(crate) same_recipe_retries: u32,
    /// `pin`, `default`, `explicit`, or the ordered `rule[n]` that selected this recipe.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) routing_rule: String,
    /// Rolf's exact words authorizing a one-off recipe choice. Empty on routed
    /// and historical launches.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) recipe_basis: String,
    /// The request on the stable task that contains `recipe_basis`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) recipe_request: String,
    pub(crate) reason: String,
    /// The compact `<job> runs on <plain>` sentence for the board's
    /// `ade_last` token (D17 item 14), stored on the record so the ticker
    /// never rereads live config.
    pub(crate) compact_reason: String,
    /// Evidence intentionally left out before dispatch (for example a review
    /// diff that the agent reads from its checkout). Escalations retain it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source_truncation: Option<serde_json::Value>,
    /// Default machine from `[dispatch]`; empty keeps the launch local.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) machine: String,
}

/// Process identity from `pane process-info` once the agent is ready
/// (SPEC-ADE D3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct ProcessIdentity {
    pub(crate) pid: u32,
    pub(crate) argv0: String,
}

/// Identity binding compared on live reads. `terminal_id` is never stored
/// or compared (SPEC-ADE D3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct IdentityBinding {
    pub(crate) socket: String,
    pub(crate) workspace_id: String,
    pub(crate) tab_id: String,
    pub(crate) pane_id: String,
    pub(crate) cwd: String,
    /// Absent on an adopted thread (SPEC-ADE D3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) process: Option<ProcessIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent_session: Option<String>,
}

/// The executable portion of a resolved launch, passed to process creation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct RoleSpec {
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) args: Vec<String>,
    #[serde(default)]
    pub(crate) env: Vec<String>,
    #[serde(default)]
    pub(crate) ready_timeout_ms: u64,
}

/// What the evidence says failed. `Unknown` is deliberate: absence of a pane,
/// poll, or answer is not evidence about a provider or the work itself.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[value(rename_all = "snake_case")]
pub(crate) enum FailureClass {
    Provider,
    LostConnection,
    ProcessGone,
    WorkFailed,
    #[default]
    Unknown,
}

impl FailureClass {
    pub(crate) fn plain(self) -> &'static str {
        match self {
            Self::Provider => "provider failed",
            Self::LostConnection => "connection lost",
            Self::ProcessGone => "process gone",
            Self::WorkFailed => "work failed",
            Self::Unknown => "failure unknown",
        }
    }
}

/// `done`, `waiting`, or `failed` (SPEC-ADE D5).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OpKind {
    Done,
    Waiting,
    Failed,
}

/// Complete requested payload stored at reserve so a later seal needs no
/// helper memory (SPEC-ADE D5, item 32).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub(crate) enum Requested {
    Done {
        sha: String,
        report_path: String,
    },
    Waiting {
        text: String,
        #[serde(default)]
        class: FailureClass,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_kind: Option<String>,
    },
    Failed {
        failure: String,
        #[serde(default)]
        class: FailureClass,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_kind: Option<String>,
    },
}

/// Coordinator pane and attempt that must receive the sealed event
/// (SPEC-ADE D5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct Recipient {
    pub(crate) pane: String,
    pub(crate) coordinator_attempt: u32,
}

/// Op state machine (SPEC-ADE D5).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OpState {
    Reserved,
    Staged,
    Sealed,
    Abandoned,
}

/// `ops/<op id>.toml`. Op id is `<thread>-<attempt>-<n>`. The event id is
/// this op id, fixed at reserve (SPEC-ADE D5, item 32).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Op {
    pub(crate) op: String,
    pub(crate) revision: u32,
    pub(crate) thread: String,
    pub(crate) attempt: u32,
    pub(crate) kind: OpKind,
    pub(crate) recipient: Recipient,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) round: Option<String>,
    pub(crate) helper_pid: u32,
    pub(crate) requested: Requested,
    /// Fixed event id: equal to `op` (SPEC-ADE D5, item 32).
    pub(crate) event: String,
    pub(crate) state: OpState,
    pub(crate) created: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) artifact: Option<String>,
}

/// A coordinator's explicit acceptance of a resolved lane's stored report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct Attestation {
    pub(crate) coordinator: String,
    pub(crate) reason: String,
}

/// Sealed `done` payload (SPEC-ADE D5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct DonePayload {
    /// Empty only when an attested historical lane has no git folder left.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) sha: String,
    pub(crate) report_path: String,
    pub(crate) artifact: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) attestation: Option<Attestation>,
}

/// Sealed `waiting` payload (SPEC-ADE D5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct WaitingPayload {
    pub(crate) text: String,
    /// Historical payloads had only text; they truthfully load as unknown.
    #[serde(default)]
    pub(crate) class: FailureClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provider_kind: Option<String>,
}

/// Tagged event payload: `payload.done` or `payload.waiting` (SPEC-ADE D5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct EventPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) done: Option<DonePayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) waiting: Option<WaitingPayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) failed: Option<WaitingPayload>,
}

/// Immutable sealed event `events/<event id>.toml` (SPEC-ADE D5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Event {
    pub(crate) id: String,
    pub(crate) op: String,
    pub(crate) thread: String,
    pub(crate) attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) round: Option<String>,
    pub(crate) recipient: Recipient,
    pub(crate) created: String,
    pub(crate) payload: EventPayload,
}

/// Delivery journal states appended to `deliveries/<event id>.jsonl`
/// (SPEC-ADE D5).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeliveryState {
    Submitted,
    Acknowledged,
    Handled,
}

/// One line of the delivery journal (SPEC-ADE D5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DeliveryLine {
    pub(crate) event: String,
    pub(crate) state: DeliveryState,
}

/// Durable `asks/<ask id>/r<revision>.toml` written before any publication
/// (SPEC-ADE D17 item 4).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct Ask {
    pub(crate) id: String,
    pub(crate) revision: u32,
    pub(crate) project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) round: Option<String>,
    pub(crate) question: String,
    pub(crate) choices: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) what: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) means: Option<String>,
    pub(crate) asked: String,
    pub(crate) coordinator_binding: String,
}

/// The only values `publish()` accepts (SPEC-ADE D17 item 3, item 35).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HumanMessage {
    Say {
        id: String,
        what: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        means: Option<String>,
        /// The merged round this line is landing evidence for (SPEC-talk §6.1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        landed_round: Option<String>,
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
pub(crate) struct CompletionPin {
    pub(crate) event: String,
    pub(crate) attempt: u32,
    pub(crate) sha: String,
    pub(crate) artifact: String,
}

/// One admitted lane in the round manifest (SPEC-ADE D6, item 33).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct ManifestMember {
    pub(crate) thread: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pin: Option<CompletionPin>,
}

/// Authoritative admitted set. Membership is never inferred from completions
/// (SPEC-ADE D6, item 33).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct AdmissionManifest {
    pub(crate) revision: u64,
    #[serde(default)]
    pub(crate) members: Vec<ManifestMember>,
}

/// Lifecycle owned by the round record, not by events or git refs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RoundPhase {
    #[default]
    Admitting,
    PreparingReview,
    UnderReview,
    VerdictIn,
    Merging,
    Checkpointing,
    Merged,
    Abandoned,
    Diverged,
}

impl RoundPhase {
    pub(crate) fn closed(self) -> bool {
        matches!(self, Self::Merged | Self::Abandoned)
    }
}

/// Planned review outputs, saved before any branch or brief commit is written.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ReviewIntent {
    pub(crate) head: String,
    pub(crate) branch: String,
    pub(crate) brief: String,
    pub(crate) manifest_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reuse_brief: Option<String>,
}

/// A gate pinned into a round. The string shape exists only so historical
/// round records keep loading; newly opened rounds always store `Typed`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub(crate) enum PinnedGate {
    Legacy(String),
    Typed(crate::project::Gate),
}

impl PinnedGate {
    pub(crate) fn command(&self) -> &str {
        match self {
            Self::Legacy(command) => command,
            Self::Typed(gate) => &gate.command,
        }
    }

    pub(crate) fn env(&self) -> Option<&std::collections::BTreeMap<String, String>> {
        match self {
            Self::Legacy(_) => None,
            Self::Typed(gate) => Some(&gate.env),
        }
    }
}

/// `.state/rounds/r<n>.toml` owns the entire round transaction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct RoundRecord {
    #[serde(default)]
    pub(crate) phase: RoundPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) review_intent: Option<ReviewIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) merge: Option<MergeIntent>,
    /// Accepted reviewer completion; later events cannot replace this pin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) verdict: Option<CompletionPin>,
    /// The validated verdict word. Historical records without it remain
    /// readable; a merged round itself proves a MERGE verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) verdict_kind: Option<String>,
    pub(crate) round: String,
    pub(crate) branch: String,
    pub(crate) plain: String,
    /// `None` means the selected repository had no gate policy. An empty
    /// vector is an explicit gate-free policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) gates: Option<Vec<PinnedGate>>,
    pub(crate) policy_hash: String,
    pub(crate) manifest: AdmissionManifest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expected_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) manifest_hash: Option<String>,
    /// When `ha round open` wrote the record; orders name views (A3).
    #[serde(default)]
    pub(crate) opened: String,
    /// Repository the integration branch lives in, fixed at open (A3).
    #[serde(default)]
    pub(crate) repo: String,
    /// Manifest revision frozen at the review brief commit `B` (SPEC-ADE D6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) frozen_revision: Option<u64>,
    /// `review/r<n>`, created from `B` (SPEC-ADE D6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) review_branch: Option<String>,
    /// The reviewer thread whose sealed `done` sha is `V` (SPEC-ADE D6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reviewer: Option<String>,
    /// What `round advance` last announced for this round (a verdict or a
    /// gone reviewer), so each state is announced once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) announced: Option<String>,
    /// Current coordinator action, owned by this round rather than the inbox.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) attention: String,
    /// How many reviewer starts `advance` has tried and failed for this round
    /// (a refused start or a reviewer whose agent never came up). The retry is
    /// bounded by `round::MAX_REVIEWER_START_FAILURES` (E3/D1).
    #[serde(default)]
    pub(crate) reviewer_start_failures: u32,
    /// REJECT verdicts observed before the round eventually merged. `None`
    /// means this historical round predates outcome tracking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rejections: Option<u32>,
    /// Human-supplied reason for deliberately ending an unmergeable round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) abandoned_reason: Option<String>,
    /// Publication and installation policy pinned at open. Historical rounds
    /// default to no post-merge effects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) push_remote: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) published: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) install_required: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) installed: bool,
    /// The round closed before each member received its durable cleanup mark.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) cleanup_pending: bool,
}

impl RoundRecord {
    /// Whether this round currently carries a lane. The manifest preserves
    /// historical admissions, but abandoning the round releases its members.
    pub(crate) fn carries(&self, thread: &str) -> bool {
        self.phase != RoundPhase::Abandoned
            && self
                .manifest
                .members
                .iter()
                .any(|member| member.thread == thread)
    }
}

/// Checkpoint intent bound to `V` and the HANDOFF payload hash
/// (SPEC-ADE D6, item 34).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct CheckpointIntent {
    pub(crate) parent: String,
    pub(crate) op: String,
    pub(crate) payload_hash: String,
}

/// Merge transaction phase (SPEC-ADE D6, item 34).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MergePhase {
    Intent,
    Merged,
    Checkpointed,
    MergeDiverged,
}

/// Merge/checkpoint transaction embedded in the owning round record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct MergeIntent {
    pub(crate) op: String,
    pub(crate) expected_old: String,
    pub(crate) candidate: String,
    pub(crate) verdict: String,
    pub(crate) phase: MergePhase,
    /// The commit the integration branch held after merging V in: `V` on a
    /// fast-forward, otherwise a merge commit whose first parent is the moved
    /// head (SPEC-ADE D6, item 34). The checkpoint commits on top of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) merged: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) checkpoint: Option<CheckpointIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) head: Option<String>,
}

/// Talk inbound request states (SPEC-ADE D18 item 2, item 35).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TalkRequestState {
    Queued,
    Submitted,
    Uncertain,
    Accepted,
}

/// `inbound { request, state }` on the talk journal (SPEC-ADE D18 item 2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TalkInbound {
    pub(crate) request: String,
    pub(crate) state: TalkRequestState,
    pub(crate) recipient: Recipient,
}

// --------------------------------------------------------------- plan card

/// The seven end-result kinds and their fixed display sentence
/// (SPEC-talk §2.7, §6.5). The coordinator picks one; the file stores it.
pub(crate) const PLAN_KINDS: &[(&str, &str)] = &[
    ("screen", "A screen you open."),
    ("command", "A command you run."),
    ("background", "A program that runs underneath."),
    ("document", "A document."),
    ("picture", "A picture."),
    ("number", "A number."),
    ("finding", "A finding."),
];

/// The fixed sentence for a stored kind, or `None` for an unknown one.
pub(crate) fn plan_kind_sentence(kind: &str) -> Option<&'static str> {
    PLAN_KINDS.iter().find(|(k, _)| *k == kind).map(|(_, v)| *v)
}

/// `state` on one plan step (SPEC-talk §6.5). It is a persisted projection of
/// the bound work, never a coordinator-supplied status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum StepState {
    #[default]
    Left,
    Running,
    Done,
}

impl StepState {
    pub(crate) fn word(self) -> &'static str {
        match self {
            StepState::Left => "left",
            StepState::Running => "running",
            StepState::Done => "done",
        }
    }
}

/// One ordered plan step (SPEC-talk §6.5). `tasks`, `threads` and `rounds` are
/// required work, not related discussions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub(crate) struct PlanStep {
    pub(crate) id: String,
    pub(crate) text: String,
    pub(crate) state: StepState,
    /// Stable task ids. New steps bind tasks; the other fields keep historical
    /// cards readable without inventing task records.
    #[serde(default)]
    pub(crate) tasks: Vec<String>,
    pub(crate) threads: Vec<String>,
    pub(crate) rounds: Vec<String>,
}

/// `<project>/plan.toml` (SPEC-talk §6.5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub(crate) struct Plan {
    pub(crate) schema: u32,
    pub(crate) revision: u64,
    pub(crate) next_step: u64,
    pub(crate) goal: String,
    pub(crate) kind: String,
    pub(crate) what_you_get: String,
    pub(crate) does: String,
    pub(crate) steps: Vec<PlanStep>,
}

// ---------------------------------------------------------- decision log

/// The four decision classes (SPEC-talk §6.6). `what-you-get`, `money` and
/// `undo` need human authority; `routine` does not.
pub(crate) const DECISION_CLASSES: &[&str] = &["what-you-get", "money", "undo", "routine"];

/// One complete line of `<project>/decisions.jsonl` (SPEC-talk §6.6). Nullable
/// fields stay present as `null` so an old reader sees the shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub(crate) struct Decision {
    pub(crate) schema: u32,
    pub(crate) seq: u64,
    pub(crate) id: String,
    pub(crate) at: String,
    pub(crate) line: String,
    pub(crate) class: String,
    pub(crate) key: Option<String>,
    pub(crate) basis: Option<String>,
    pub(crate) replaces: Option<String>,
    pub(crate) request: Option<String>,
    pub(crate) overturned: Option<DecisionOverturn>,
}

/// An append-only change to a decision; the original line remains in the log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DecisionOverturn {
    pub(crate) by: String,
    pub(crate) at: String,
    pub(crate) reason: String,
}

/// A `--basis` reference: an existing human message (`request:<id>`) or a
/// current, nonzero answered ask (`ask:<id>@<revision>`) (SPEC-talk §6.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthorityRef {
    Request(String),
    Ask { id: String, revision: u32 },
}

impl AuthorityRef {
    pub(crate) fn parse(text: &str) -> Option<AuthorityRef> {
        if let Some(id) = text.strip_prefix("request:") {
            if id.is_empty() {
                return None;
            }
            return Some(AuthorityRef::Request(id.to_string()));
        }
        let rest = text.strip_prefix("ask:")?;
        let (id, revision) = rest.rsplit_once('@')?;
        Some(AuthorityRef::Ask {
            id: id.to_string(),
            revision: revision.parse().ok()?,
        })
    }

    pub(crate) fn as_str(&self) -> String {
        match self {
            AuthorityRef::Request(id) => format!("request:{id}"),
            AuthorityRef::Ask { id, revision } => format!("ask:{id}@{revision}"),
        }
    }
}

/// One JSON object on `talk/journal.jsonl` (SPEC-ADE D18 items 2 and 6).
#[cfg(test)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TalkJournalRecord {
    pub(crate) seq: u64,
    pub(crate) inbound: TalkInbound,
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
                    attestation: None,
                }),
                waiting: None,
                failed: None,
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
                    ..Default::default()
                }),
                failed: None,
            },
        });
    }

    #[test]
    fn historical_failure_payload_is_unknown_and_typed_provider_roundtrips() {
        let old: WaitingPayload = toml::from_str("text = \"no evidence\"\n").unwrap();
        assert_eq!(old.class, FailureClass::Unknown);
        assert!(old.provider_kind.is_none());
        both(&WaitingPayload {
            text: "fetch failed".into(),
            class: FailureClass::Provider,
            provider_kind: Some("unreachable".into()),
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
            id: "s-1".into(),
            what: "A lane is done.".into(),
            means: None,
            landed_round: None,
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
    fn plan_step_decision_and_reference_roundtrip() {
        let plan = Plan {
            schema: 1,
            revision: 8,
            next_step: 6,
            goal: "I want to build a trading bot with Jeff.".into(),
            kind: "screen".into(),
            what_you_get: "A screen you open.".into(),
            does: "It shows pretend trades and lets you stop them.".into(),
            steps: vec![
                PlanStep {
                    id: "s-1".into(),
                    text: "Choose what the screen will show.".into(),
                    state: StepState::Done,
                    tasks: vec!["job-0001".into()],
                    threads: vec!["t-0041".into()],
                    rounds: vec![],
                },
                PlanStep {
                    id: "s-2".into(),
                    text: "Show pretend trades.".into(),
                    state: StepState::Running,
                    tasks: vec![],
                    threads: vec!["t-0043".into(), "t-0044".into()],
                    rounds: vec!["r1".into()],
                },
            ],
        };
        both(&plan);
        // An old record without the newer fields still deserializes.
        let old: Plan = toml::from_str(
            "schema = 1\nrevision = 1\nnext_step = 2\ngoal = \"A goal.\"\n[[steps]]\nid = \"s-1\"\ntext = \"One step.\"\n",
        )
        .unwrap();
        assert_eq!(old.steps[0].state, StepState::Left);
        assert!(old.steps[0].threads.is_empty());

        let decision = Decision {
            schema: 1,
            seq: 2,
            id: "d-0002".into(),
            at: "2026-09-19T12:18:00Z".into(),
            line: "I will show more detail beside each choice.".into(),
            class: "routine".into(),
            key: Some("change-q-example".into()),
            basis: None,
            replaces: Some("d-0001".into()),
            request: Some("q-example".into()),
            overturned: None,
        };
        json_roundtrip(&decision);
        let text = serde_json::to_string(&decision).unwrap();
        assert!(text.contains("\"basis\":null"), "{text}");
        let old: Decision = serde_json::from_str(
            "{\"schema\":1,\"seq\":1,\"id\":\"d-0001\",\"at\":\"x\",\"line\":\"A line.\",\"class\":\"routine\"}",
        )
        .unwrap();
        assert!(old.key.is_none() && old.replaces.is_none());

        assert_eq!(
            AuthorityRef::parse("request:q-example"),
            Some(AuthorityRef::Request("q-example".into()))
        );
        assert_eq!(
            AuthorityRef::parse("ask:a-3@2"),
            Some(AuthorityRef::Ask {
                id: "a-3".into(),
                revision: 2
            })
        );
        assert_eq!(AuthorityRef::parse("ask:a-3"), None);
        assert_eq!(AuthorityRef::parse("nonsense"), None);
    }

    #[test]
    fn recipe_and_launch_roundtrip() {
        let recipe = Recipe {
            kind: "cursor".into(),
            args: vec!["--model".into(), "cursor-grok-4.6-xhigh".into()],
            env: vec![],
            ready_timeout_ms: 30_000,
            provider: "cursor".into(),
            capabilities: vec!["pictures".into()],
            enabled: true,
            plain: "the usual coding helper".into(),
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
            skill_hash: "aa".into(),
            recipe_id: "agy_gemini_flash".into(),
            escalations: 0,
            same_recipe_retries: 0,
            routing_rule: "default".into(),
            recipe_basis: String::new(),
            recipe_request: String::new(),
            reason: "this task runs on the web research helper, the usual choice.".into(),
            compact_reason: "this task runs on the web research helper".into(),
            source_truncation: None,
            machine: "oci".into(),
        });
    }

    #[test]
    fn historical_records_with_removed_routing_fields_still_load() {
        let json: Launch = serde_json::from_value(serde_json::json!({
            "recipe_id": "old",
            "strength": 3,
            "assessment": {"score": 1},
            "decision": {"recipe": "old"},
            "low_confidence": true,
            "routing_hash": "aa"
        }))
        .unwrap();
        assert_eq!(json.recipe_id, "old");

        let toml: Launch = toml::from_str(
            r#"recipe_id = "old"
strength = 3
assessment = { score = 1 }
decision = { recipe = "old" }
low_confidence = true
routing_hash = "aa"
"#,
        )
        .unwrap();
        assert_eq!(toml.recipe_id, "old");

        let dispatch: serde_json::Value = serde_json::from_str(
            r#"{"kind":"pick","assessment":{"score":1},"decision":{"recipe":"old"},"low_confidence":true,"routing_hash":"aa"}"#,
        )
        .unwrap();
        assert_eq!(dispatch["decision"]["recipe"], "old");

        let round: RoundRecord = toml::from_str(
            r#"round = "r1"
branch = "main"
plain = "This round checks old records."
policy_hash = "old"
routing_hash = "aa"
assessment = { score = 1 }

[manifest]
revision = 1
members = []
"#,
        )
        .unwrap();
        assert_eq!(round.round, "r1");
    }

    #[test]
    fn round_merge_checkpoint_and_talk_roundtrip() {
        both(&RoundRecord {
            round: "r1".into(),
            branch: "main".into(),
            plain: "The first round lands the contracts.".into(),
            gates: Some(vec![PinnedGate::Legacy("cargo test --locked".into())]),
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
            announced: Some("verdict:MERGE".into()),
            reviewer_start_failures: 0,
            ..Default::default()
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
