# Linux fault tools

The wall creates disposable local and loopback-SSH ADE instances for fault reproduction. It is a privileged development tool, not ordinary macOS/Linux setup and not a portable clean-clone gate.

## Host requirements

The controller assumes a dedicated Linux host with:

- systemd, cgroups and writable service definitions under `/etc/systemd/system/`.
- Root access through sudo, account/group management tools, mount tools and tmpfs.
- OpenSSH server at `/usr/sbin/sshd`, SSH client and `ssh-keygen`.
- Python 3, Bash, Git, Node/npm, Herdr and built ADE executables.
- The host account and paths expected by the scripts. Several checks and gate paths explicitly use `/home/ubuntu/`; these are host assumptions, not paths to substitute blindly.
- Free loopback ports and enough resources for the selected instances.

Review `wall`, `instance.py`, `gate` and the fault scripts before running them. They create accounts, services and mounts. Do not run them on a shared workstation or a host with occupied wall slots. These privileged campaigns were not run during the publication cleanup.

## Instances

Commands accept `--instance N` before the verb, with N from 1 to 8. Omitting it selects the default `wall`/`wallbox` accounts under `/home/wall`, ports 22285/22286 and saved machine `wall-box`.

- Instances 1 to 4 are manual slots. Use only a free, explicitly assigned slot.
- Instances 5 to 8 are reserved together for `tools/wall/gate`. Manual use requires the matching lock under `/home/ubuntu/.cache/herdr-wall-gate/`.

Instance N uses `wallN`/`wallboxN`, `/home/wall-N`, ports `22285+2*N` and `22286+2*N`, and machine `wall-box-N`. Each has separate roots, sockets, control files, releases and logs. Thread IDs resolve only in that instance's records.

On a prepared host, from the checkout:

```bash
cargo build --bins --locked
sudo tools/wall/wall list
sudo tools/wall/wall --instance 1 install \
  --build "${CARGO_TARGET_DIR:-target}/debug" \
  --herdr /path/to/herdr --node /path/to/node --npm /path/to/npm
sudo tools/wall/wall --instance 1 reset
sudo tools/wall/wall --instance 1 enter
```

Replace the three executable paths with actual host installations. Install stages the named build, npm package, pinned Pi download and tools. It does not import host npm configuration, provider credentials or SSH identities. Reset stops only that instance, wipes its scratch filesystem and rebuilds both accounts. Capture evidence before reset. Reset retains the shared sandbox login.

Each scratch home uses a 2 GiB tmpfs. Services have memory and task limits; numbered instances also share aggregate limits. Host homes and sockets are hidden, host filesystems are read-only and the scratch home is writable. Accounts cannot use sudo or signal the host account. Outbound networking remains available. This is filesystem, account and resource isolation, not a VM or a network-denial firewall. Never give sandbox agents the host controller's sudo access.

## Scripted and authenticated agents

After installing and resetting a free instance:

```bash
sudo tools/wall/wall --instance 1 enter 'python3 "$HOME/tools/guest.py" lane'
sudo tools/wall/wall --instance 1 enter 'python3 "$HOME/tools/guest.py" remote-lane'
```

The helper uses the real prompt hook and records, then starts lanes through ADE. Its ordinary `open_project()` path retains the historical D27 assisted-start workaround after a failed open. The strict path, `open_project(strict=True)`, requires an unassisted start and is used by D27 and the gate journey. During reset, the helper also removes only the exact empty OpenCode provider declaration from its Codex-only sandbox. These are fixture workarounds, not production fixes. The gate treats either workaround finding as a failure.

Scripted agents commit scratch changes and call the real sealing command. They do not simulate Pi's full TUI, provider sessions or semantic review. The ordinary fault reviewer rejects. The gate has a separate narrow fixture check that can merge exact expected files; that is not evidence of arbitrary real-model acceptance. The remote path uses a local bare Git repository and real loopback SSH, not GitHub.

Control lives in each account's `$HOME/control.json`:

```json
{"delay_ms":500,"hold":["before-seal"],"fail_at":"","finish":"done","seal_delay_ms":0}
```

Release a held phase with `touch "$HOME/release/<phase>"`. `fail_at` exits the scripted agent at that checkpoint. `seal_delay_ms` delays real sealing Git calls to expose a mid-seal interruption. Ordinary Git calls are not delayed.

For authenticated Pi, Rolf must enter an installed instance, run `pi`, and complete `/login openai-codex` locally. Check that the `wall_real` recipe's model is available before dispatch. No provider call is established by a scripted run. Sandbox credentials live in `/var/lib/herdr-wall-auth/auth.json`, outside reset paths. All wall accounts share that sandbox credential store, not a host login. `sudo tools/wall/wall logout` clears it for every instance. Do not export or commit it.

## Faults and regressions

Examples for a free installed instance:

```bash
sudo tools/wall/wall --instance 1 fault kill-pane t-0001 --when t-0001 before-seal
sudo tools/wall/wall --instance 1 fault disconnect 5
sudo tools/wall/wall --instance 1 fault reboot 2
sudo tools/wall/wall --instance 1 fault fill
sudo tools/wall/wall --instance 1 fault corrupt .state/threads/t-0001.toml truncate
sudo tools/wall/wall --instance 1 fault install /absolute/alternate-build-directory
```

Use an actual recorded thread ID. `--box` selects the second account. `--after` and `--when` control timing; a checkpoint is not proof of an atomic-operation boundary. Process faults verify the account and checkout instead of accepting arbitrary host PIDs. Fill refuses a non-tmpfs target. Corruption refuses paths outside the selected project. Install changes only the sandbox ADE binary and ticker.

The reboot fault restarts sandbox services, not the host kernel. Disconnect pauses loopback SSH transport, not every possible network path. Clock injection is unavailable because ADE has no shared injectable clock. Linux campaigns cannot establish macOS behavior or physical reboot recovery.

See [additional faults](faults/README.md) and [active regressions and gate contract](regressions/README.md). Historical reports and their standalone reproducers remain under `findings/`. They retain observations for their named builds, including failing findings. External evidence archives are not included. Historical discovery reproducers are not part of the passing gate's regression discovery set.

## Evidence and local checks

```bash
sudo tools/wall/wall --instance 1 evidence /absolute/new/evidence-directory
sudo tools/wall/wall --instance 1 prove /absolute/new/proof-directory /absolute/alternate-build-directory
python3 -m unittest discover -s tools/wall -p 'test_*.py' -v
sudo tools/wall/prove-isolation --instance 3 --peer 4 /absolute/new/isolation-proof
```

The isolation proof resets both named slots. Never run it on occupied instances. Use evidence directories outside the checkout. The gate stores captures under `/home/ubuntu/.cache/herdr-wall-gate/wall-gate-*`; these files are not included in Git.

Exports exclude provider auth, SSH keys, npm caches and Pi sessions. Logs and reports may still contain private agent text. Inspect captures before sharing. Unit checks exercise controller boundaries with local fixtures and mocks; they do not establish that a privileged campaign or authenticated journey passed.
