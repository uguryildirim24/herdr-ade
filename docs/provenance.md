# Provenance and contributions

## Upstream foundation

The local Git history starts at `20c8c276323dbc4fd15bda3d38226a9f7c2a192b`, titled `Build herdr-projects: projects for herdr`, attributed to Elias Stravik. Its README names `eliasstravik/herdr-projects` as the installation source. Its MIT license contains the copyright notice for Elias Stravik that remains unchanged in ADE's [LICENSE](../LICENSE).

That initial tree already implements a coordinator, parallel agent threads, Git worktrees, shared project records, remote threads and an overview. These are upstream work, not original contributions by Rolf. The next upstream commit, `6e2bd76`, adds the coordinator-managed task list.

This identification comes from the local repository, not a network comparison with an upstream checkout. Upstream availability and its current contents were not checked. Herdr itself is an external prerequisite. ADE does not vendor or relicense Herdr.

## Later ADE development

Rolf's repository continues that foundation as Herdr ADE. The history records commits under `uguryildirim24` and agent identities. AI coding agents did much of the implementation under Rolf's direction. Commit attribution identifies a development record, not proof that Rolf personally wrote each line or that an agent's report is correct.

These examples separate later development from the initial foundation:

| Change in the ADE development history | Local commit |
| --- | --- |
| Rename Herdr Projects to Herdr ADE and extend its command surface | `5b63284` |
| Add the pinned Pi runtime integration | `4594e01` |
| Replace earlier routing with editable bounded policy | `44587ad` |
| Derive project work from stable task records | `742dda6` |
| Add a project-bound Rundown pane | `4959d96` |
| Replace rounds with durable repository pile review | `96bae01` |

These are examples, not a complete authorship inventory. They do not attribute upstream architecture to Rolf. Rolf directed the current cleanup: preserve runtime behavior and tracked evidence, keep private data out of the tree, preserve applicable notices and document checks without claiming authenticated success. Agents performed the mechanical checks described in [Operations](operations.md#cleanup-verification). No completed personal code audit by Rolf is claimed.

The evidence can be inspected without changing Git state:

```bash
git show 20c8c276323dbc4fd15bda3d38226a9f7c2a192b:README.md
git show 20c8c276323dbc4fd15bda3d38226a9f7c2a192b:LICENSE
git log --follow -- LICENSE
git show --format=short --stat 5b63284 4594e01 44587ad 742dda6 4959d96 96bae01
```

## Release review

The local evidence resolves the source of the existing copyright notice. It does not record Rolf's confirmation of the fork relationship, his exact contribution scope or which implementation reviews he completed. Rolf must approve this attribution account before publication. Until then, it is a source-backed history description, not a confirmed personal contribution statement. No upstream endorsement is implied.
