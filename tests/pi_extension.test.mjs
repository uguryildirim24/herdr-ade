import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import guard from '../extensions/herdr-pi-guard.ts';

function fixture() {
  const cwd = mkdtempSync(join(tmpdir(), 'ade-pi-hook-'));
  mkdirSync(join(cwd, '.pi'));
  const script = join(cwd, 'hook.cjs');
  writeFileSync(script, `let data = ''; process.stdin.on('data', c => data += c);
process.stdin.on('end', () => {
  require('fs').appendFileSync(process.argv[2], JSON.stringify({phase: process.argv[3], ...JSON.parse(data)}) + '\\n');
  if (process.argv[3] === 'stop') console.log(JSON.stringify({decision: 'block', reason: 'Run ha say before you finish'}));
});`);
  const log = join(cwd, 'hooks.jsonl');
  const argv = [process.execPath, script, log];
  writeFileSync(join(cwd, '.pi/herdr-ade-hooks.json'), JSON.stringify({
    pane: 'w1:p1', prompt: [...argv, 'prompt'], stop: [...argv, 'stop'],
  }));
  const handlers = new Map();
  const sent = [];
  const pi = {
    on(name, fn) { handlers.set(name, [...(handlers.get(name) || []), fn]); },
    sendUserMessage(text) { sent.push(text); },
    events: { emit() {} },
  };
  const notices = [];
  const ctx = {cwd, isProjectTrusted: () => false, sessionManager: {getSessionId: () => 'pi-session-1'}, hasUI: true, ui: {notify(text) { notices.push(text); }}};
  const previous = process.env.HERDR_PANE_ID;
  process.env.HERDR_PANE_ID = 'w1:p1';
  guard(pi);
  return {ctx, sent, notices,
    async emit(name, event) { for (const fn of handlers.get(name) || []) await fn(event, ctx); },
    lines: () => readFileSync(log, 'utf8').trim().split('\n').map(JSON.parse), close() {
    if (previous === undefined) delete process.env.HERDR_PANE_ID;
    else process.env.HERDR_PANE_ID = previous;
    rmSync(cwd, {recursive: true, force: true});
  }};
}

test('submitted pane text reaches the prompt hook with pi session and cwd', async () => {
  const f = fixture();
  try {
    await f.emit('input', {source: 'interactive', text: 'Start a Fable lane.'});
    assert.deepEqual(f.notices, []);
    assert.deepEqual(f.lines(), [{phase: 'prompt', prompt: 'Start a Fable lane.', session_id: 'pi-session-1', cwd: f.ctx.cwd}]);
  } finally { f.close(); }
});

test('stop block is returned to the model without recording the correction as a request', async () => {
  const f = fixture();
  try {
    await f.emit('input', {source: 'interactive', text: 'Check this.'});
    await f.emit('agent_settled', {});
    assert.deepEqual(f.sent, ['Run ha say before you finish']);
    await f.emit('input', {source: 'extension', text: f.sent[0]});
    assert.deepEqual(f.lines().map(row => row.phase), ['prompt', 'stop']);
  } finally { f.close(); }
});

test('provider failure never prompts a correction or invokes the stop hook', async () => {
  const f = fixture();
  try {
    await f.emit('input', {source: 'interactive', text: 'Check this.'});
    await f.emit('agent_end', {messages: [{role: 'assistant', stopReason: 'error', errorMessage: 'provider unavailable'}]});
    await f.emit('agent_settled', {});
    assert.deepEqual(f.sent, []);
    assert.deepEqual(f.lines().map(row => row.phase), ['prompt']);
  } finally { f.close(); }
});

test('unbound panes cannot invoke project hooks', async () => {
  const f = fixture();
  try {
    process.env.HERDR_PANE_ID = 'other';
    await f.emit('input', {source: 'interactive', text: 'not this project'});
    await f.emit('agent_settled', {});
    assert.throws(() => f.lines());
  } finally { f.close(); }
});
