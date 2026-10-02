import type { On, SessionCompactInput, SessionCompactResult } from 'claude-code';
import { test, expect, mock } from 'claude-code/testing';

const home = '/Users/test';
const cwd = `${home}/.herdr-ade/demo`;
const input: SessionCompactInput = { trigger: 'auto', messages: [{ role: 'user', text: 'live transcript', toolUses: [] }] };
const fallback: SessionCompactResult = { messages: [{ role: 'assistant', text: 'core summary', toolUses: [] }] };
const usage = { input_tokens: 1, output_tokens: 1, cache_creation_input_tokens: 0, cache_read_input_tokens: 1 };

function setup(on: On, directory = cwd, coordinator = true) {
  mock.env(on, { HOME: home });
  const clock = mock.clock(on, { now: Date.parse('2026-10-02T12:34:56.000Z') });
  on('session.cwd', () => ({ value: directory }));
  on('fs.exists', (_, e) => ({ value: coordinator && e.path === `${cwd}/.state/coordinator.json` }));
  on('session.compact', (_, e) => {
    expect(e).toEqual(inputForFallback);
    return fallback;
  });
  let inputForFallback = input;
  return { clock, setInput: (e: SessionCompactInput) => { inputForFallback = e; } };
}

for (const directory of ['/tmp/demo', `${home}/.herdr-ade/demo/nested`, `${home}/.herdr-ade`]) {
  test(`passes through outside a coordinator folder: ${directory}`, async ($, on) => {
    setup(on, directory);
    expect(await $.session.compact(input)).toEqual(fallback);
  });
}

test('passes through a project with no coordinator record', async ($, on) => {
  setup(on, cwd, false);
  expect(await $.session.compact(input)).toEqual(fallback);
});

test('precompute is skipped without forking or running core', async ($, on) => {
  setup(on);
  expect(await $.session.compact({ ...input, trigger: 'precompute' })).toEqual({ skip: 'coordinator handoff is built at compaction' });
});

test('subagent compaction passes through even inside a coordinator folder', async ($, on) => {
  const e = { ...input, agentId: 'agent-1' };
  setup(on).setInput(e);
  expect(await $.session.compact(e)).toEqual(fallback);
});

test('unanswered fork falls back', async ($, on) => {
  setup(on);
  on('model.fork', () => ({ value: { isAnswered: false, reason: 'nothing-to-fork' } }));
  expect(await $.session.compact(input)).toEqual(fallback);
});

test('nonzero ha exit falls back', async ($, on) => {
  setup(on);
  on('model.fork', () => ({ value: { isAnswered: true, text: 'note', usage } }));
  on('process.run', (_, e) => {
    expect(e.argv).toEqual([`${home}/.local/bin/ha`, 'handoff', 'demo', '--note-file', '-']);
    expect(e.init?.stdin).toBe('note');
    return { value: { exitCode: 1, stdout: 'partial output', stderr: 'error', isStdoutTruncated: false, isStderrTruncated: false } };
  });
  expect(await $.session.compact(input)).toEqual(fallback);
});

test('successful ha exit with truncated stdout falls back', async ($, on) => {
  setup(on);
  on('model.fork', () => ({ value: { isAnswered: true, text: 'note', usage } }));
  on('process.run', () => ({ value: { exitCode: 0, stdout: 'partial handoff', stderr: '', isStdoutTruncated: true, isStderrTruncated: false } }));
  let announced = false;
  on('ui.toast', () => { announced = true; return { value: undefined }; });
  expect(await $.session.compact(input)).toEqual(fallback);
  expect(announced).toBe(false);
});

test('slow fork cannot wedge compaction', async ($, on) => {
  const { clock } = setup(on);
  on('model.fork', async () => {
    await clock.sleep(180_000);
    return { value: { isAnswered: true, text: 'late note', usage } };
  });
  const result = $.session.compact(input);
  await clock.settle();
  await clock.advance(120_000);
  expect(await result).toEqual(fallback);
});

for (const trigger of ['manual', 'auto', 'plugin'] as const) {
  test(`${trigger}: passes the note to ha and returns its single user handoff`, async ($, on) => {
    setup(on);
    on('model.fork', (_, e) => {
      for (const section of ['Goal', 'Settled', 'Open', 'The one next action', 'Traps already hit', "aren't in the harness records", 'previous handoff', 'what changed since']) {
        expect(e.prompt).toContain(section);
      }
      expect(e.prompt).toContain('Compaction instructions: keep next action');
      return { value: { isAnswered: true, text: 'fresh session note', usage } };
    });
    on('process.run', (_, e) => {
      expect(e.argv).toEqual([`${home}/.local/bin/ha`, 'handoff', 'demo', '--note-file', '-']);
      expect(e.init?.stdin).toBe('fresh session note');
      return { value: { exitCode: 0, stdout: 'complete harness handoff\n', stderr: '', isStdoutTruncated: false, isStderrTruncated: false } };
    });
    for (const operation of ['fs.list', 'fs.read', 'fs.write'] as const) {
      on(operation, () => { throw new Error('mod must not assemble or save the handoff'); });
    }
    let toast = '';
    on('ui.toast', (_, e) => { toast = e.text; return { value: undefined }; });
    const result = await $.session.compact({ ...input, trigger, instructions: 'keep next action' });
    expect(result).toEqual({ messages: [{ role: 'user', text: 'complete harness handoff\n', toolUses: [] }] });
    expect(toast).toBe('coordinator handoff written');
  });
}

for (const stdout of ['', ' \n\t']) {
  test(`empty ha output falls back: ${JSON.stringify(stdout)}`, async ($, on) => {
    setup(on);
    on('model.fork', () => ({ value: { isAnswered: true, text: 'note', usage } }));
    on('process.run', () => ({ value: { exitCode: 0, stdout, stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }));
    expect(await $.session.compact(input)).toEqual(fallback);
  });
}

test('a process failure falls back rather than installing the handoff', async ($, on) => {
  setup(on);
  on('model.fork', () => ({ value: { isAnswered: true, text: 'note', usage } }));
  on('process.run', () => { throw new Error('disk full'); });
  expect(await $.session.compact(input)).toEqual(fallback);
});
