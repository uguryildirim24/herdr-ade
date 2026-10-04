# D46: second-open ownership

Install the candidate into a numbered instance you have reserved, then run:

```sh
tools/wall/regressions/D46/repro --instance N
```

The repro starts from that instance's reset and resets it again on exit. It uses
real `ha open`, close and second open, real Herdr and the wall's declared scripted
Pi. A sandbox-only Herdr wrapper records tokens immediately before agent start,
forwards the start, then returns `agent_name_not_found`. EXPECTED/ACTUAL output
checks that the failed second start already owned project/coordinator tokens,
without expiry. Missing pre-start tokens exit nonzero on the unfixed image.

No provider/TUI claim. Linux cannot run the Mac-only journey. Its actual shutdown
driver is covered by `failed_second_open_keeps_start_tokens_and_tears_down_every_spawned_process`
and the live Mac failed-open journey. Do not reset an instance used by another lane.
