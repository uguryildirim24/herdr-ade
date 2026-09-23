// herdr-pi-guard:version=4
// Plugin-owned pi extension (SPEC-pi v2 §3.7, §3.10). Written by
// setup and every harness install; doctor compares the complete file.
//
// A provider error does not reach the model, so the model cannot report it.
// This extension is the deterministic reporter: a settled
// provider failure becomes `herdr:blocked` on the pane (never idle, never
// `done`) plus one typed `ha failed` event per class per ten minutes.
//
// It never runs a login, never retries, and never runs `ha done`.
// @ts-nocheck

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { spawn } from "node:child_process";

// pi has no hooks.json. ADE installs these commands in the coordinator's
// project; the global extension runs them only in its bound pane.
function commands(ctx) {
  const pane = process.env.HERDR_PANE_ID;
  // ADE owns this file, not pi's project-local extension loader; pi's normal
  // defaultProjectTrust is `never` for ADE projects.
  if (!pane) return null;
  try {
    const value = JSON.parse(readFileSync(join(ctx.cwd, ".pi/herdr-ade-hooks.json"), "utf8"));
    if (value.pane !== pane) return null;
    if (![value.prompt, value.activate, value.stop].every((argv) =>
      Array.isArray(argv) && argv.length > 1 && argv.every((arg) => typeof arg === "string"))) return null;
    return value;
  } catch {
    return null;
  }
}

function hook(argv, payload) {
  return new Promise((resolve, reject) => {
    const child = spawn(argv[0], argv.slice(1), { stdio: ["pipe", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => child.kill(), 10000);
    child.on("error", reject);
    child.stdout.on("data", (chunk) => { stdout += chunk; });
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    child.on("close", (code) => {
      clearTimeout(timer);
      if (code !== 0) reject(new Error(`ADE hook exited ${code}: ${stderr.trim()}`));
      else resolve(stdout);
    });
    child.stdin.end(JSON.stringify(payload));
  });
}

const CLASSES = ["limit", "login", "unreachable", "error"];
const THROTTLE_MS = 10 * 60 * 1000;
const DETAIL_MAX = 120;

export default function (pi) {
  // reason key -> label; the pane reports blocked while any reason is active.
  const reasons = new Map();
  let pendingError = null; // { message, status }
  let lastStatus = null;
  const lastSentAt = new Map(); // class -> epoch ms
  let activeHook = null;
  let corrections = 0;
  let firstMessagePending = false;
  const queued = []; // prompts submitted during streaming, not yet active turns

  pi.on("input", async (event, ctx) => {
    // sendUserMessage corrections are extension delivery, not Rolf's words.
    if (event.source === "extension") return;
    const config = commands(ctx);
    if (!config) return;
    const payload = {
      prompt: event.text,
      session_id: ctx.sessionManager.getSessionId(),
      cwd: ctx.cwd,
      queued: !!event.streamingBehavior,
    };
    try {
      await hook(config.prompt, payload);
      if (payload.queued) queued.push({ config, payload });
      else {
        queued.length = 0;
        firstMessagePending = true;
        activeHook = { config, payload };
        corrections = 0;
      }
    } catch (error) {
      if (ctx.hasUI) ctx.ui.notify(`ADE prompt check failed: ${error}`, "error");
      return { action: "handled" }; // never let an unrecorded request through
    }
  });

  pi.on("message_start", async (event, ctx) => {
    if (event.message?.role !== "user") return;
    if (firstMessagePending) {
      firstMessagePending = false;
      return;
    }
    if (!queued.length) return;
    const next = queued.shift();
    try {
      await hook(next.config.activate, next.payload);
      activeHook = next;
      corrections = 0;
    } catch (error) {
      activeHook = null;
      if (ctx.hasUI) ctx.ui.notify(`ADE queued turn check failed: ${error}`, "error");
      throw error;
    }
  });

  pi.on("agent_settled", async (_event, ctx) => {
    if (!activeHook) return;
    // A failed provider call cannot publish a receipt. Do not turn its
    // failure into a corrective model prompt (and another provider call).
    if (pendingError) {
      activeHook = null;
      return;
    }
    const { config, payload } = activeHook;
    try {
      const output = await hook(config.stop, payload);
      const response = JSON.parse(output || "null");
      if (response?.decision === "block" && typeof response.reason === "string") {
        // Unlike Claude's native Stop hook, pi needs an explicit next message.
        // Bound the retry just as the native hook does; keep the failed turn
        // visible if the coordinator still does not publish a receipt.
        if (++corrections <= 3) pi.sendUserMessage(response.reason, { deliverAs: "followUp" });
        else {
          activeHook = null;
          if (ctx.hasUI) ctx.ui.notify(response.reason, "error");
        }
      } else {
        activeHook = null;
      }
    } catch (error) {
      activeHook = null;
      if (ctx.hasUI) ctx.ui.notify(`ADE stop check failed: ${error}`, "error");
      throw error;
    }
  });

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
      /unauthorized|forbidden|credentials|api key|apikey|invalid token|invalid_token|expired token|token expired|token has expired|refresh token|access token|re-login|relogin|log in again|login expired|not_ready|not ready/.test(
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

  // Only an ADE lane (HERDR_ADE_LAUNCH set) reports; the pane's `blocked`
  // state is raised either way.
  async function runWaiting(label, cls, provider) {
    const env = process.env || {};
    if (!env.HERDR_ADE_LAUNCH) {
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
      await pi.exec(
        "ha",
        ["failed", "--class", "provider", "--provider-kind", cls, text],
        { timeout: 5000 },
      );
    } catch {
      // A failed report never stops pi; the pane is blocked either way.
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
