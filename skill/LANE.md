# Lane brief

You are one lane of a herdr ADE project. A coordinator gave you the task at the end of this brief. Other lanes may work in parallel; their screens are not your coordination channel.

- Continue the current attempt. A repeated skill call does not restart the task.
- Do the task in your recorded git folder. A code task gets a worktree; a task with no code repository gets a project-owned git folder. If something is missing, report exactly what is missing instead of guessing.
- The pile review publishes the integration branch. On the cloud box, `ha done` publishes only your lane branch; on the Mac, `done` does not publish.
- Keep technical detail in reports. Messages meant for Rolf should be short and clear.
- Do not add a refusal, flag, config key or check the brief didn't ask for. If you think one is needed, say so in the report instead.
- Do not edit project memory. Put durable lessons in your report for the coordinator to decide.
- Never add a throwaway tab or pane to your lane workspace or to the watched session. Run visual checks and probes in the isolated session `herdr --session scratch-<lane id> ...` on your lane's machine. When done, run `herdr session stop scratch-<lane id>` and `herdr session delete scratch-<lane id>`; resolve also removes a leftover session.

## If this attempt fails

When your approach fails, preserve the git folder and run:

```text
ha failed "<what failed, what you tried, and the evidence>"
```

This seals a `work_failed` event. The harness follows the matched routing rule's same-recipe retries while keeping the worktree. Provider and connection failures are classified by the harness and never switch model; a gone process restarts; missing evidence is reported as unknown and waits for the coordinator. Do not keep editing after sealing it. Use `waiting` for missing input, not for a failed approach.

## Finish

Run `ha done` only when the task's acceptance is met; if blocked or acceptance is not met, run `ha waiting "<what blocks it>"` instead, because a done seal counts as finished and opens the steps after it.

Commit the finished work and write the report at the path named by your brief.
Pass that same path to `done`: an absolute path or a path relative to the
git folder is accepted, but the report must be a file inside that folder.

```text
ha done --report <report path from the brief> --sha <commit sha>
```

If you must stop for input, keep your work and run:

```text
ha waiting "<what is missing>"
```

Both commands create a durable event. Do not type a separate DONE or WAITING line.

## On the cloud box

A brief that says you run on a named cloud box runs on a saved machine, not on the home machine. The box's default-root command is `ha`; run the birth line's skill call first.

- The code branch is pinned to an exact Mac commit. The frozen brief arrives separately in `.herdr-project/<project>-<id>/brief.md`; it is never committed to the code repository.
- Every kind logs in once per machine. If your kind is not signed in on the box, stop and run `ha waiting "<kind> is not signed in on the box"`; never copy a Mac credential across.
- Commit your code, then run `ha done`. It publishes your lane branch to the recorded remote and verifies the ref before sealing. If publishing fails, it tells you what went wrong; retry `ha done` after the issue is resolved.
- `ha done`, `ha waiting` and `ha failed` seal locally on the box. They do not deliver to the coordinator; the Mac courier carries the sealed event home.
- After a reboot or a resize the old attempt is GONE. The coordinator restarts you from the exact start line in the brief; never resume a cold shell.
