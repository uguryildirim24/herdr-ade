// herdr-pi-guard:version=1
// Plugin-owned pi extension (SPEC-pi v2 §3.7, §3.10). Written by
// `herdr-pi setup`; doctor checks the marker on the first line.
//
// A provider error does not reach the model, so the model cannot type
// `ha waiting`. This extension is the deterministic reporter: a settled
// provider failure becomes `herdr:blocked` on the pane (never idle, never
// `done`) plus one `ha waiting` line per class per ten minutes.
//
// It never runs a login, never retries, and never runs `ha done`.
// The classification patterns mirror `src/pi/limits.rs`; keep them together.
// @ts-nocheck

const CLASSES = ["limit", "login", "unreachable", "error"];
const THROTTLE_MS = 10 * 60 * 1000;
const DETAIL_MAX = 120;

export default function (pi) {
  // reason key -> label; the pane reports blocked while any reason is active.
  const reasons = new Map();
  let pendingError = null; // { message, status }
  let lastStatus = null;
  const lastSentAt = new Map(); // class -> epoch ms

  function first120(text) {
    const one = String(text ?? "").replace(/\s+/g, " ").trim();
    return one.length > DETAIL_MAX ? one.slice(0, DETAIL_MAX) : one;
  }

  function classify(message, status) {
    const text = `${message ?? ""} ${status ?? ""}`.toLowerCase();
    if (
      status === 429 ||
      /rate limit|rate-limit|ratelimit|too many requests|usage limit|quota|billing|available balance|insufficient balance/.test(
        text,
      )
    ) {
      return "limit";
    }
    if (
      status === 401 ||
      status === 403 ||
      /unauthorized|forbidden|credentials|api key|apikey|token|re-login|relogin|log in again|login expired|not_ready|not ready/.test(
        text,
      )
    ) {
      return "login";
    }
    if (
      /connection refused|econnrefused|fetch failed|enotfound|timeout|timed out|network|socket hang up|connection reset|unreachable/.test(
        text,
      )
    ) {
      return "unreachable";
    }
    return "error";
  }

  function providerName(ctx) {
    try {
      const provider = ctx?.model?.provider;
      return typeof provider === "string" && provider.length > 0 ? provider : "pi";
    } catch {
      return "pi";
    }
  }

  function emit(active, label) {
    try {
      pi.events.emit("herdr:blocked", active ? { active: true, label } : { active: false });
    } catch {
      // The herdr state hook may not be loaded; a failed report never stops pi.
    }
  }

  function raise(key, label) {
    const wasEmpty = reasons.size === 0;
    reasons.set(key, label);
    if (wasEmpty) {
      emit(true, label);
    }
  }

  function clear(key) {
    if (!reasons.delete(key)) {
      return;
    }
    if (reasons.size === 0) {
      emit(false);
    }
  }

  function clearAll() {
    if (reasons.size > 0) {
      reasons.clear();
      emit(false);
    }
    pendingError = null;
    lastStatus = null;
  }

  async function runWaiting(label, cls, provider) {
    const env = process.env || {};
    const launch = env.HERDR_ADE_LAUNCH;
    if (!launch) {
      return;
    }
    const now = Date.now();
    const sentAt = lastSentAt.get(cls) || 0;
    if (now - sentAt < THROTTLE_MS) {
      return;
    }
    lastSentAt.set(cls, now);
    const text = `${provider} ${cls}: ${first120(label)}`;
    try {
      const result = await pi.exec("ha", ["waiting", text], { timeout: 5000 });
      if (result && result.code === 0) {
        return;
      }
    } catch {
      // `ha` is not on PATH (before ADE lands); fall through to herdr.
    }
    try {
      await notifyParent(pi, text);
    } catch {
      // Best effort only; a missing parent is not an error in the guard.
    }
  }

  function messageOf(message) {
    if (typeof message?.errorMessage === "string" && message.errorMessage.length > 0) {
      return message.errorMessage;
    }
    const content = message?.content;
    if (Array.isArray(content)) {
      const text = content
        .map((part) => (typeof part?.text === "string" ? part.text : ""))
        .join(" ")
        .trim();
      if (text.length > 0) {
        return text;
      }
    }
    return typeof content === "string" ? content : "provider error";
  }

  pi.on("after_provider_response", (event) => {
    try {
      lastStatus = typeof event?.status === "number" ? event.status : null;
    } catch {
      lastStatus = null;
    }
  });

  pi.on("agent_end", (event) => {
    try {
      const messages = Array.isArray(event?.messages) ? event.messages : [];
      let outcome = null;
      for (let i = messages.length - 1; i >= 0; i -= 1) {
        if (messages[i]?.role === "assistant") {
          outcome = messages[i];
          break;
        }
      }
      if (!outcome) {
        return;
      }
      if (outcome.stopReason === "error") {
        pendingError = { message: messageOf(outcome), status: lastStatus };
      } else {
        // A retry that succeeded settles without the error.
        pendingError = null;
      }
    } catch {
      pendingError = null;
    }
  });

  pi.on("agent_settled", async (_event, ctx) => {
    if (!pendingError) {
      return;
    }
    const provider = providerName(ctx);
    const cls = classify(pendingError.message, pendingError.status);
    if (!CLASSES.includes(cls)) {
      return;
    }
    const detail = first120(pendingError.message);
    const label = pendingError.status
      ? `${provider} ${cls} HTTP ${pendingError.status}: ${detail}`
      : `${provider} ${cls}: ${detail}`;
    raise("provider", label);
    await runWaiting(detail, cls, provider);
  });

  // Typing in the pane is the recovery: it clears the block, and the model
  // gets the message on its next turn.
  pi.on("agent_start", () => {
    clearAll();
  });

  pi.on("input", () => {
    clearAll();
  });

  pi.on("ui_prompt_start", (event) => {
    try {
      const title = first120(event?.title || event?.kind || "waiting for you");
      raise("ui", `waiting for you: ${title}`);
    } catch {
      raise("ui", "waiting for you");
    }
  });

  pi.on("ui_prompt_end", () => {
    clear("ui");
  });
}

// Before ADE lands, read this pane's parent token and tell the coordinator
// through herdr itself (SPEC-pi v2 §3.7).
async function notifyParent(pi, text) {
  const paneId = process.env.HERDR_PANE_ID;
  if (!paneId) {
    return;
  }
  const herdr = process.env.HERDR_BIN_PATH || "herdr";
  const list = await pi.exec(herdr, ["agent", "list", "--json"], { timeout: 5000 });
  if (!list || list.code !== 0) {
    return;
  }
  let parsed;
  try {
    parsed = JSON.parse(list.stdout);
  } catch {
    return;
  }
  const agents = parsed?.result?.agents || [];
  const self = agents.find((agent) => agent?.pane_id === paneId);
  const parent = self?.tokens?.parent;
  if (!parent) {
    return;
  }
  const name = self?.name || paneId;
  await pi.exec(herdr, ["agent", "prompt", parent, `WAITING ${name} ${text}`], { timeout: 5000 });
}
