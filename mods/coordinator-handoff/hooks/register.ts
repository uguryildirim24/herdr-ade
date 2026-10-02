import type { Register, Timer } from 'claude-code';

const NOTE_PROMPT = `Write a fresh coordinator handoff, concrete and without narration, using exactly these sections:
- Goal
- Settled: findings with the number or file that proves each
- Open: each with why it is open
- The one next action
- Traps already hit
- Commands and facts worked out this session that aren't in the harness records
The transcript may begin with a previous handoff. Carry forward only items still open or still true, and write about what changed since. Do not summarize an old summary wholesale.`;

export const register: Register = (on) => {
  on('session.compact', async ($, e, next) => {
    if (e.agentId !== undefined) return next(e);
    try {
      const home = await $.env.get('HOME');
      if (!home) return next(e);
      const cwd = (await $.session.cwd()).replace(/\/$/, '');
      const root = `${home}/.herdr-ade/`;
      if (!cwd.startsWith(root)) return next(e);
      const slug = cwd.slice(root.length);
      if (!slug || slug.includes('/')) return next(e);
      if (!(await $.fs.exists(`${cwd}/.state/coordinator.json`))) return next(e);
      // Core would cache a summary here and bypass the next real compaction.
      if (e.trigger === 'precompute') return { skip: 'coordinator handoff is built at compaction' };

      // $ calls in flight do not consume the hook's own-time budget. Fork has
      // no timeout option: race a host timer, without a budget-consuming sleep.
      // A late fork only resolves this promise; it cannot write or install data.
      let timer: Timer | undefined;
      let note;
      try {
        const deadline = new Promise<never>((_, reject) => {
          timer = $.clock.after(120_000, () => reject(new Error('handoff fork timed out')));
        });
        note = await Promise.race([
          $.model.fork({ prompt: NOTE_PROMPT + (e.instructions ? `\nCompaction instructions: ${e.instructions}` : '') }),
          deadline,
        ]);
      } finally {
        timer?.cancel();
      }
      if (!note.isAnswered) return next(e);
      const handoff = await $.process.run(
        [`${home}/.local/bin/ha`, 'handoff', slug, '--note-file', '-'],
        { stdin: note.text },
      );
      if (handoff.exitCode !== 0 || handoff.isStdoutTruncated || !handoff.stdout.trim()) return next(e);
      await $.ui.toast('coordinator handoff written');
      return { messages: [{ role: 'user', text: handoff.stdout, toolUses: [] }] };
    } catch {
      return next(e);
    }
  });
};
