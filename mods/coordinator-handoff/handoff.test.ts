import type { On, SessionCompactInput, SessionCompactResult } from 'claude-code';
import { test, expect, mock } from 'claude-code/testing';

const home = '/Users/test';
const cwd = `${home}/.herdr-ade/demo`;
const timestamp = '2026-10-02T12:34:56.000Z';
const input: SessionCompactInput = { trigger: 'auto', messages: [{ role: 'user', text: 'live transcript', toolUses: [] }] };
const fallback: SessionCompactResult = { messages: [{ role: 'assistant', text: 'core summary', toolUses: [] }] };
const usage = { input_tokens: 1, output_tokens: 1, cache_creation_input_tokens: 0, cache_read_input_tokens: 1 };

function setup(on: On, directory = cwd, coordinator = true) {
  mock.env(on, { HOME: home });
  const clock = mock.clock(on, { now: Date.parse(timestamp) });
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
    expect(e.argv).toEqual([`${home}/.local/bin/ha`, 'context', 'demo', '--peek']);
    return { value: { exitCode: 1, stdout: '', stderr: 'error', isStdoutTruncated: false, isStderrTruncated: false } };
  });
  expect(await $.session.compact(input)).toEqual(fallback);
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
  test(`${trigger}: writes and returns a single user handoff`, async ($, on) => {
    setup(on);
    on('model.fork', (_, e) => {
      for (const section of ['Goal', 'Settled', 'Open', 'The one next action', 'Traps already hit', "aren't in the harness records", 'previous handoff', 'what changed since']) {
        expect(e.prompt).toContain(section);
      }
      return { value: { isAnswered: true, text: 'fresh session note', usage } };
    });
    on('process.run', (_, e) => {
      expect(e.argv).toEqual([`${home}/.local/bin/ha`, 'context', 'demo', '--peek']);
      return { value: { exitCode: 0, stdout: 'harness records', stderr: '', isStdoutTruncated: false, isStderrTruncated: false } };
    });
    on('fs.list', (_, e) => {
      expect(e.path).toBe(`${cwd}/.state/requests`);
      return { value: Array.from({ length: 14 }, (_, n) => ({ name: `q-${String(14 - n).padStart(2, '0')}.json`, kind: 'file' as const, size: 0, mtimeMs: 0, isLink: false })) };
    });
    on('fs.read', (_, e) => {
      const id = e.path.split('/').pop()!.replace('.json', '');
      return { value: JSON.stringify({ id, at: timestamp, text: `verbatim ${id}\n  with **formatting**` }) };
    });
    let savedPath = '';
    let savedText = '';
    on('fs.write', (_, e) => { savedPath = e.path; savedText = e.text; return { value: undefined }; });
    let toast = '';
    on('ui.toast', (_, e) => { toast = e.text; return { value: undefined }; });
    const result = await $.session.compact({ ...input, trigger });
    expect(savedPath).toBe(`${cwd}/.state/handoffs/${timestamp}.md`);
    expect(result).toEqual({ messages: [{ role: 'user', text: savedText, toolUses: [] }] });
    expect(savedText).toContain('fresh session note');
    expect(savedText).toContain('harness records');
    expect(savedText).toContain(savedPath);
    expect(savedText).not.toContain('verbatim q-01');
    expect(savedText).not.toContain('verbatim q-02');
    let previous = -1;
    for (let n = 3; n <= 14; n++) {
      const text = `verbatim q-${String(n).padStart(2, '0')}\n  with **formatting**`;
      expect(savedText).toContain(text);
      const position = savedText.indexOf(text);
      expect(position).toBeGreaterThan(previous);
      previous = position;
    }
    expect(toast).toBe(`coordinator handoff written: ${savedPath}`);
  });
}

test('a write failure falls back rather than installing the handoff', async ($, on) => {
  setup(on);
  on('model.fork', () => ({ value: { isAnswered: true, text: 'note', usage } }));
  on('process.run', () => ({ value: { exitCode: 0, stdout: 'context', stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }));
  on('fs.list', () => ({ value: [] }));
  on('fs.write', () => { throw new Error('disk full'); });
  expect(await $.session.compact(input)).toEqual(fallback);
});
