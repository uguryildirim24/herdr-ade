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
  on('session.compact', () => fallback);
  return clock;
}

test('nested folders do not inherit coordinator compaction', async ($, on) => {
  setup(on, `${cwd}/nested`);
  expect(await $.session.compact(input)).toEqual(fallback);
});

test('a project with no coordinator record passes through', async ($, on) => {
  setup(on, cwd, false);
  expect(await $.session.compact(input)).toEqual(fallback);
});

test('subagent compaction passes through inside a coordinator folder', async ($, on) => {
  setup(on);
  expect(await $.session.compact({ ...input, agentId: 'agent-1' })).toEqual(fallback);
});

test('a nonzero process exit cannot install partial output', async ($, on) => {
  setup(on);
  on('model.fork', () => ({ value: { isAnswered: true, text: 'note', usage } }));
  on('process.run', () => ({ value: { exitCode: 1, stdout: 'partial output', stderr: 'error', isStdoutTruncated: false, isStderrTruncated: false } }));
  expect(await $.session.compact(input)).toEqual(fallback);
});

test('blank process output cannot become a handoff', async ($, on) => {
  setup(on);
  on('model.fork', () => ({ value: { isAnswered: true, text: 'note', usage } }));
  on('process.run', () => ({ value: { exitCode: 0, stdout: ' \n\t', stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }));
  expect(await $.session.compact(input)).toEqual(fallback);
});

test('truncated process output cannot become a handoff', async ($, on) => {
  setup(on);
  on('model.fork', () => ({ value: { isAnswered: true, text: 'note', usage } }));
  on('process.run', () => ({ value: { exitCode: 0, stdout: 'partial handoff', stderr: '', isStdoutTruncated: true, isStderrTruncated: false } }));
  let announced = false;
  on('ui.toast', () => { announced = true; return { value: undefined }; });
  expect(await $.session.compact(input)).toEqual(fallback);
  expect(announced).toBe(false);
});

test('slow fork cannot wedge compaction', async ($, on) => {
  const clock = setup(on);
  on('model.fork', async () => {
    await clock.sleep(180_000);
    return { value: { isAnswered: true, text: 'late note', usage } };
  });
  const result = $.session.compact(input);
  await clock.settle();
  await clock.advance(120_000);
  expect(await result).toEqual(fallback);
});

test('ha alone assembles the handoff and the mod returns its message protocol', async ($, on) => {
  setup(on);
  on('model.fork', (_, e) => {
    expect(e.prompt).toContain('Prioritize operating facts over activity history.');
    expect(e.prompt).toContain('one entry per fact, one location per fact.');
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
  expect(await $.session.compact(input)).toEqual({ messages: [{ role: 'user', text: 'complete harness handoff\n', toolUses: [] }] });
});

test('a process failure cannot install a handoff', async ($, on) => {
  setup(on);
  on('model.fork', () => ({ value: { isAnswered: true, text: 'note', usage } }));
  on('process.run', () => { throw new Error('disk full'); });
  expect(await $.session.compact(input)).toEqual(fallback);
});
