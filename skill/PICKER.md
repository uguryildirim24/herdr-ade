# Picker: how a coordinator pins a helper

The picker reads the task text when a lane, reviewer, drafter or critic starts. It may move the launch to another allowed helper only when one of its yes-or-no questions is clearly yes. It never changes a launch after the fact, and it never runs for a coordinator or for Pro.

The picker ships switched off (`[roles] resolver = "off"` in `~/.config/herdr-ade/config.toml`). `shadow` records what it would have chosen and launches the usual helper. `jev` (launch the pick) is not in this plugin round: the config is refused with `resolver_mode_unavailable` until a second recording month shows the picker beats the simple table.

## Pinning one helper for a task

Pass `--recipe` on the verb that starts the work. This is your word for that launch; the picker is skipped.

```
herdr-ade thread start demo --title "Read the vendor pages" --repo /srv/demo --task-file - --recipe agy_gemini_flash
herdr-ade dialogue start auth-review --drafter drafter --critic critic --plain "..." --recipe claude_opus_high
```

The id must be in that role's `allowed` list in the safety file. An id outside the list is refused with `recipe_not_allowed`; an unknown id is refused with `recipe_unknown`.

- Use `--recipe` when the job is clear to you: a review that needs the strongest judge, a lookup that reads the web, a second opinion on a draft.
- Give `--role` when the work is not a lane: `--role reviewer`, `--role critic`, `--role drafter`, `--role research`. The role selects the allowed set and the questions; it is not a model pick.
- Do not write a helper name into the task text and expect it to stick. The picker warns `task names a model; pass --recipe to pin it` and resolves as usual.
- The picker has one ask per lane and one per side of a dialogue, and two hundred asks per project per day. Past the ceiling it uses the usual helper and records `daily_cap`.

## What a launch records

Every launch records the helper that actually ran (`recipe_id`, `kind`, `args`), the mode (`resolver`), the question and its probability when one fired (`gate`, `gate_p`), what the picker would have chosen in `shadow` mode (`jev_pick`), a short checked reason sentence, and the fallback when the picker did not decide. The ticker launches from that record only. A restart reuses it and never asks again unless you pass `--recipe` to `thread restart`.

The reason sentence on the board is at most eighty characters and uses a job name, not a product name: "this task runs on the web research helper". A fallback other than `shadow`, `pin`, `resolver_off`, `single_row` or `no_gate` also writes an inbox item `jev-fallback`.

## What the picker never does

- It never picks a helper outside the role's allowed list, and never a helper that is `enabled = false`.
- It never sends a task from a project that has not opted in (`jev = true` in `PROJECT.md`), and never from a project it does not recognize.
- It never blocks a start for long: three seconds in total, one retry on a busy service. A timeout, a missing key or a missing curl means the usual helper runs and the board says so.
- It never starts a stronger model. Sol and Astra sit under `escalate` for a stalled lane, with Rolf asked first.
