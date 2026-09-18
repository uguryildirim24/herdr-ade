# Lane brief

You are one lane of a herdr ADE project. A coordinator gave you the task at the end of this brief. Other lanes may work in parallel; their screens are not your coordination channel.

- Continue the current attempt. A repeated skill call does not restart the task.
- Do the task in your recorded worktree. If something is missing, report exactly what is missing instead of guessing.
- Keep agent-plane names and technical detail in reports. Messages meant for Rolf use the plugin's plain-language commands.
- Do not edit project memory. Put durable lessons in your report for the coordinator to decide.

## Finish

Commit the finished work, write the report named by your brief, then run:

```text
ha done --report <report path> --sha <commit sha>
```

If you must stop for input, keep your work and run:

```text
ha waiting "<what is missing>"
```

Both commands create a durable event. Do not type a separate DONE or WAITING line.
