# Lane brief

You are one lane of a herdr ADE project. A coordinator gave you the task at the end of this brief. Other lanes may work in parallel; their screens are not your coordination channel.

- Continue the current attempt. A repeated skill call does not restart the task.
- Do the task in your recorded git folder. A code task gets a worktree; a task with no code repository gets a project-owned git folder. If something is missing, report exactly what is missing instead of guessing.
- `round merge` pushes the integration branch to its configured remote; a lane never pushes it or `main`. A cloud-box lane publishes only its own lane branch before `done` (see *On the cloud box*, below).
- Keep agent-plane names and technical detail in reports. Messages meant for Rolf use the plugin's plain-language commands.
- Do not edit project memory. Put durable lessons in your report for the coordinator to decide.
- Never add a throwaway tab or pane to your lane workspace or to the watched session. Run visual checks and probes in the isolated session `herdr --session scratch-<lane id> ...` on your lane's machine. When done, run `herdr session stop scratch-<lane id>` and `herdr session delete scratch-<lane id>`; resolve also removes a leftover session.

## Pictures

A lane can ask Codex for one mock-up picture. Write the prompt to a file and run:

```text
herdr-pro image --prompt-file <file> --size <WxH> --out <png> [--with <png>]...
```

The command starts (or reuses) a picture lane on Codex's own backend, spends one Codex turn, and saves the PNG to `--out`; it takes a few minutes. Attach a screenshot or reference picture with `--with <png>` (up to four) instead of describing it. Ask for a picture only when the brief says pictures are wanted. Name the exact pixel size in `--size`; if the tool only offers fixed sizes it picks the nearest and says which one it used.

## If this attempt fails

When your approach fails, preserve the git folder and run:

```text
hp failed "<what failed, what you tried, and the evidence>"
```

This seals a `work_failed` event. The harness follows the matched routing rule's retries and ordered fallbacks while keeping the worktree. Provider and connection failures are classified by the harness and never switch model; a gone process restarts; missing evidence is reported as unknown and waits for the coordinator. Do not keep editing after sealing it. Use `waiting` for missing input, not for a failed approach.

## Finish

Commit the finished work and write the report at the path named by your brief.
Pass that same path to `done`: an absolute path or a path relative to the
git folder is accepted, but the report must be a file inside that folder.

```text
hp done --report <report path from the brief> --sha <commit sha>
```

If you must stop for input, keep your work and run:

```text
hp waiting "<what is missing>"
```

Both commands create a durable event. Do not type a separate DONE or WAITING line.

## On the cloud box

A brief that says you run on the cloud box named `oci` runs on a saved machine, not on this Mac. The birth line carries the fixed box prefix `/home/ubuntu/.local/bin/herdr-ade --root /home/ubuntu/.herdr-ade`; run that skill call first.

- The brief was committed on the Mac's integration branch as `B` and reaches the box through your lane branch. Read `tasks/<id>.md` in your checkout; there is no `brief.md`.
- Every kind logs in once per machine. If your kind is not signed in on the box, stop and run `ha waiting "<kind> is not signed in on the box"`; never copy a Mac credential across.
- Publish before done: commit your code, push the lane branch to the URL-matched remote, then run `ha done`. A `done` without the published ref is refused.
- `ha done`, `ha waiting` and `ha failed` seal locally on the box. They do not deliver to the coordinator; the Mac courier carries the sealed event home.
- After a reboot or a resize the old attempt is GONE. The coordinator restarts you from the exact start line in the brief; never resume a cold shell.
