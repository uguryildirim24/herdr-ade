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
      const context = await $.process.run([`${home}/.local/bin/ha`, 'context', slug, '--peek']);
      if (context.exitCode !== 0) return next(e);
      const requestsDir = `${cwd}/.state/requests`;
      const records = (await $.fs.list(requestsDir))
        .filter(entry => entry.kind === 'file' && entry.name.endsWith('.json'))
        .sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0)
        .slice(-12);
      const requests: string[] = [];
      for (const record of records) {
        const request = JSON.parse(await $.fs.read(`${requestsDir}/${record.name}`));
        requests.push(`### ${request.id} · ${request.at}\n\n${request.text}`);
      }
      const utc = new Date(await $.clock.now()).toISOString();
      const path = `${cwd}/.state/handoffs/${utc}.md`;
      const handoff = `# Coordinator handoff\n\nThis replaced a compaction at ${utc}. Saved at ${path}.\n\n## Session note\n\n${note.text}\n\n## Harness context\n\n${context.stdout}\n\n## Rolf's latest messages (verbatim, oldest first)\n\n${requests.join('\n\n')}\n`;
      await $.fs.write(path, handoff);
      await $.ui.toast(`coordinator handoff written: ${path}`);
      return { messages: [{ role: 'user', text: handoff, toolUses: [] }] };
    } catch {
      return next(e);
    }
  });
};
