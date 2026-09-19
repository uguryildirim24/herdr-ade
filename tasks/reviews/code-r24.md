+++
verdict = "MERGE"
round = "r24"
candidate = "b13ab88b8ca59ac8599b6a93fdf059febcaffda1"
manifest_hash = "321c9d921713632d36f290956f9010f8086a197b0e1aba3644c422007a3f8298"
policy_hash = "a90dfd009a8bc5abf13e9b75c22944d273735fc31cbfedd79d047b7727a9e8d0"
gates = []
+++

# Round r24

## t-0050 — lanes and reviewers go to the box by default

MERGE. A lane or reviewer now uses the machine named by its role when its repository has a box clone. Other roles, tasks without repositories, and roles without the machine switch remain local. An explicit machine remains strict, while a default placement falls back with the promised plain line.

I fixed two review defects. First, only pi launches checked whether the box was reachable, so a native lane in an outage failed later during provisioning instead of falling back. Every box placement now proves readiness before choosing the box; pi retains its provider check and other kinds use a short SSH probe. Second, model recipes could still name a machine even though this round makes the role row the single switch. Recipe-level machine placement is removed, and an absent role machine now means local.

The round froze no PROJECT.md gates, hence `gates = []`. As additional review checks, formatting passed and all 535 tests passed. The machine entries in `~/.config/herdr-ade/config.toml` remain the installation switch outside this candidate.
