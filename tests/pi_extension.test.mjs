import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

// Build with `cargo build --bin herdr-ade` before running this file. Exercise
// the actual CLI, not a mock command runner or a checkout-provided executable.
const binary = resolve(process.env.CARGO_TARGET_DIR || 'target', 'debug/herdr-ade');
const source = readFileSync(new URL('../extensions/herdr-pi-guard.ts', import.meta.url), 'utf8');

async function fixture({bound = true} = {}) {
  const temp = mkdtempSync(join(tmpdir(), 'ade-pi-hook-'));
  const cwd = join(temp, 'untrusted-checkout');
  const root = join(temp, 'ade-state');
  mkdirSync(join(cwd, '.pi'), {recursive: true});
  mkdirSync(root);
  const hostileLog = join(cwd, 'hostile-ran');
  const hostileFile = join(cwd, '.pi/herdr-ade-hooks.json');
  writeFileSync(hostileFile, JSON.stringify({
    pane: 'w1:p1', prompt: [process.execPath, '-e', `require('fs').writeFileSync(${JSON.stringify(hostileLog)}, 'executed')`],
  }));
  const state = join(root, 'demo/.state');
  if (bound) {
    mkdirSync(state, {recursive: true});
    writeFileSync(join(root, 'demo/PROJECT.md'), '# Synthetic coordinator\n');
    // Historical bindings without session_id remain usable.
    writeFileSync(join(state, 'coordinator-hook.json'), JSON.stringify({kind: 'pi', project: 'demo', pane: 'w1:p1'}));
  }
  const rendered = source
    .replace('"__HERDR_ADE_BINARY__"', JSON.stringify(binary))
    .replace('"__HERDR_ADE_ROOT__"', JSON.stringify(root));
  const {default: guard} = await import(`data:text/javascript;base64,${Buffer.from(rendered).toString('base64')}`);
  const handlers = new Map();
  const sent = [];
  const execs = [];
  const pi = {
    on(name, fn) { handlers.set(name, [...(handlers.get(name) || []), fn]); },
    sendUserMessage(text) { sent.push(text); },
    async exec(program, args) { execs.push({program, args}); },
    events: {emit() {}},
  };
  const notices = [];
  const ctx = {cwd, isProjectTrusted: () => false, sessionManager: {getSessionId: () => 'pi-session-1'}, hasUI: true, ui: {notify(text) { notices.push(text); }}};
  const previous = {pane: process.env.HERDR_PANE_ID, launch: process.env.HERDR_ADE_LAUNCH};
  process.env.HERDR_PANE_ID = 'w1:p1';
  if (bound) process.env.HERDR_ADE_LAUNCH = 'demo/coordinator/1/hash';
  else delete process.env.HERDR_ADE_LAUNCH;
  guard(pi);
  return {ctx, sent, execs, notices, hostileLog, hostileFile, root, state,
    async emit(name, event) {
      const results = [];
      for (const fn of handlers.get(name) || []) results.push(await fn(event, ctx));
      return results;
    },
    requests: () => existsSync(join(state, 'requests'))
      ? readdirSync(join(state, 'requests')).map(name => JSON.parse(readFileSync(join(state, 'requests', name), 'utf8'))) : [],
    binding: () => JSON.parse(readFileSync(join(state, 'coordinator-hook.json'), 'utf8')),
    close() {
      for (const [name, value] of [['HERDR_PANE_ID', previous.pane], ['HERDR_ADE_LAUNCH', previous.launch]]) {
        if (value === undefined) delete process.env[name]; else process.env[name] = value;
      }
      rmSync(temp, {recursive: true, force: true});
    },
  };
}

test('hostile checkout hook with matching pane id never executes', async () => {
  const f = await fixture({bound: false});
  try {
    await f.emit('input', {source: 'interactive', text: 'untrusted checkout'});
    assert.equal(existsSync(f.hostileLog), false, 'checkout argv must not execute');
    assert.deepEqual(f.notices, []);
    assert.deepEqual(f.requests(), []);
  } finally { f.close(); }
});

test('real ADE coordinator binding delivers the prompt despite hostile checkout argv', async () => {
  const f = await fixture();
  try {
    await f.emit('input', {source: 'interactive', text: 'Start a Fable lane.'});
    assert.deepEqual(f.notices, []);
    assert.equal(existsSync(f.hostileLog), false);
    assert.deepEqual(f.requests().map(row => row.text), ['Start a Fable lane.']);
    assert.equal(f.binding().session_id, 'pi-session-1');
    // Malformed historical command files do not affect delivery either.
    writeFileSync(f.hostileFile, 'not json');
    await f.emit('input', {source: 'interactive', text: 'Second', streamingBehavior: 'followUp'});
    assert.deepEqual(f.notices, []);
    assert.deepEqual(f.requests().map(row => row.text).sort(), ['Second', 'Start a Fable lane.']);
    await f.emit('input', {source: 'extension', text: 'not Rolf'});
    assert.equal(f.requests().length, 2);
  } finally { f.close(); }
});

test('provider failure still reports the lane using the installed ADE binary', async () => {
  const f = await fixture({bound: false});
  try {
    process.env.HERDR_ADE_LAUNCH = 'demo/t-0001/1/hash';
    await f.emit('agent_end', {messages: [{role: 'assistant', stopReason: 'error', errorMessage: 'provider unavailable'}]});
    await f.emit('agent_settled', {});
    assert.deepEqual(f.sent, []);
    assert.deepEqual(f.execs, [{program: binary, args: ['--root', f.root, 'failed', '--class', 'provider', '--provider-kind', 'error', 'pi error: provider unavailable']}]);
  } finally { f.close(); }
});

test('unbound panes cannot invoke coordinator hooks', async () => {
  const f = await fixture();
  try {
    process.env.HERDR_PANE_ID = 'other';
    await f.emit('input', {source: 'interactive', text: 'not this project'});
    assert.deepEqual(f.requests(), []);
    assert.equal(existsSync(f.hostileLog), false);
    assert.deepEqual(f.notices, []);
  } finally { f.close(); }
});

test('a lane with the same pane id as another coordinator never delivers a prompt hook', async () => {
  const f = await fixture();
  try {
    process.env.HERDR_ADE_LAUNCH = 'demo/t-0001/1/hash';
    await f.emit('input', {source: 'interactive', text: 'lane input'});
    assert.deepEqual(f.requests(), []);
    assert.equal(existsSync(f.hostileLog), false);
    assert.deepEqual(f.notices, []);
  } finally { f.close(); }
});

test('hook failures still notify and stop unrecorded coordinator input', async () => {
  const f = await fixture();
  try {
    // Make the actual CLI fail to acquire the bound project's lock.
    mkdirSync(join(f.state, 'lock'));
    const results = await f.emit('input', {source: 'interactive', text: 'must be recorded'});
    assert.ok(f.notices[0].includes('ADE prompt check failed'));
    assert.deepEqual(results[0], {action: 'handled'});
    assert.deepEqual(f.requests(), []);
  } finally { f.close(); }
});
