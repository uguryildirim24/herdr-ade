#!/usr/bin/env python3
"""Guest-side reliability scenarios; invoked only by run-scenario."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import tomllib

HOME = Path.home()
SESSION = "scratch-t-0819"
SOCKET = HOME / ".config/herdr/sessions" / SESSION / "herdr.sock"
LOG = HOME / "runs/herdr-contract.jsonl"


def record(**row):
    with LOG.open("a") as output:
        output.write(json.dumps(row, ensure_ascii=False) + "\n")
    print(json.dumps(row, ensure_ascii=False), flush=True)


def call(*args, timeout=45):
    result = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
    record(argv=list(args), rc=result.returncode, stdout=result.stdout, stderr=result.stderr)
    return result


def herdr(*args):
    return call("herdr", "--session", SESSION, *args)


def boot():
    (HOME / "runs").mkdir(exist_ok=True)
    os.environ["HERDR_SOCKET_PATH"] = str(SOCKET)
    os.environ["HERDR_SESSION"] = SESSION
    # The wall's bashrc sources env.sh and restores its default socket. Make
    # fixture shells and scripted lifecycle hooks use this isolated session.
    with (HOME / ".bashrc").open("a") as output:
        output.write(f"\nexport HERDR_SOCKET_PATH={SOCKET}\nexport HERDR_SESSION={SESSION}\n")
    with (HOME / "runs/contract-server.log").open("w") as output:
        subprocess.Popen(["herdr", "--session", SESSION, "server"],
                         stdin=subprocess.DEVNULL, stdout=output, stderr=output,
                         start_new_session=True)
    for _ in range(100):
        result = subprocess.run(["herdr", "--session", SESSION, "status", "server"],
                                capture_output=True)
        if result.returncode == 0:
            break
        time.sleep(.1)
    else:
        raise RuntimeError("scratch server did not become ready")
    call("ha", "--version")
    herdr("--version")
    call("sha256sum", str(HOME / "bin/herdr-ade"), str(HOME / "bin/herdr"))


def lane():
    (HOME / "control.json").write_text(json.dumps({"hold": ["working"], "delay_ms": 50}))
    result = call("python3", str(HOME / "tools/guest.py"), "lane", timeout=150)
    if result.returncode:
        raise RuntimeError("fixture lane did not start")
    path = HOME / ".herdr-ade/wall/.state/threads/t-0001.toml"
    for _ in range(200):
        row = tomllib.loads(path.read_text())
        if not row.get("prompt_pending", True):
            return path, row
        time.sleep(.1)
    raise RuntimeError("fixture brief was not acknowledged")


def large_prompt():
    path, row = lane()
    # Exercise idle delivery, not the separately tracked working-turn delivery.
    herdr("pane", "report-agent", row["pane_id"], "--source", "pi", "--agent", "pi",
          "--state", "idle", "--seq", "100000")
    herdr("agent", "list")
    note = HOME / "large-note.txt"
    note.write_text("x" * 131072)
    result = call("ha", "thread", "prompt", "wall", row["id"], "--text-file", str(note))
    row = tomllib.loads(path.read_text())
    summary = [{k: len(v.encode()) if k == "text" else v for k, v in item.items()}
               for item in row.get("follow_ups", [])]
    record(record=str(path), follow_ups=summary)
    server = herdr("status", "server")
    failed = "Argument list too long" in result.stdout + result.stderr
    stuck = any(item["state"] == "uncertain" for item in summary)
    print("EXPECTED: a file-based note is submitted, or a definite pre-submission failure is not recorded as uncertain/unreachable")
    print(f"ACTUAL: E2BIG={failed}, uncertain={stuck}, server_reachable={server.returncode == 0}")
    # A changed failure mode is not silently a pass: successful submission is
    # the positive outcome; changed refusals require review of the new evidence.
    return 1 if failed and stuck else (0 if result.returncode == 0 and any(
        item["state"] == "delivered" for item in summary) else 2)


def empty_notification():
    for title in ("", " ", "\n", "\u2003"):
        result = herdr("notification", "show", title, "--body", "wall contract")
        if result.returncode == 0 or "notification title is empty" not in result.stderr:
            raise RuntimeError("live notification contract changed; inspect evidence")
    print("EXPECTED: fake rejects the same empty/blank notification titles as the real server")
    print("ACTUAL: real returns invalid_params for all four titles; host checks the actual fake next")
    return 0


def session_selection():
    result = herdr("workspace", "create", "--cwd", str(HOME / "repo"),
                   "--label", "requested-session", "--no-focus")
    pane = json.loads(result.stdout)["result"]["root_pane"]["pane_id"]
    expected = [{key: item[key] for key in ("pane_id", "tab_id", "workspace_id", "cwd")}
                for item in json.loads(herdr("pane", "list").stdout)["result"]["panes"]]
    version = subprocess.check_output(["ha", "--version"], text=True).split()[1]
    request = {"build": version, "request": {"Courier": {"session": SESSION, "taken": []}}}
    env = dict(os.environ, HERDR_ADE_BOX_INPUT="1",
               HERDR_SOCKET_PATH=str(HOME / ".config/herdr/herdr.sock"))
    result = subprocess.run(["ha", "doctor"], input=json.dumps(request), env=env,
                            capture_output=True, text=True, timeout=45)
    record(argv=["ha", "doctor"], input=request,
           inherited_socket=env["HERDR_SOCKET_PATH"], rc=result.returncode,
           stdout=result.stdout, stderr=result.stderr)
    reply = json.loads(result.stdout)
    if result.returncode or reply.get("status") != "Ready":
        raise RuntimeError("courier helper did not return a usable observation")
    actual = reply["result"]["panes"]
    print(f"EXPECTED: requested {SESSION} has pane {pane}; snapshot={expected}")
    print(f"ACTUAL: courier reported panes={actual} with an inherited default-session socket")
    return 0 if actual == expected else 1


def boundaries():
    # Drive long generated agent names and Unicode display labels through ADE,
    # not just a hand-written equivalent of its Herdr arguments.
    name = "a-project-name-far-longer-than-herdr-agent-name-budget"
    call("ha", "new", name, "--repo", str(HOME / "repo"), "--goal", "Unicode 界🦊")
    call("ha", "open", name[:40], "--session", SESSION, timeout=90)
    call("ha", "new", "Unicode 界🦊", "--repo", str(HOME / "repo"))
    call("ha", "open", "unicode", "--session", SESSION, timeout=90)
    herdr("agent", "list")
    result = herdr("workspace", "create", "--cwd", str(HOME / "repo"),
                   "--label", "bounds", "--no-focus")
    created = json.loads(result.stdout)["result"]["root_pane"]
    pane = created["pane_id"]
    for name, kind, timeout in (("", "pi", "3001"), ("界", "pi", "3001"),
                                ("a" * 33, "pi", "3001"), ("valid", "", "3001"),
                                ("valid", "pi", "3000"), ("valid", "pi", "300001"),
                                ("valid", "pi", "18446744073709551615")):
        herdr("agent", "start", name, "--kind", kind, "--pane", pane,
              "--timeout", timeout, "--", "--wall-coordinator")
    for timeout in ("3001", "300000"):
        result = herdr("tab", "create", "--workspace", created["workspace_id"],
                       "--cwd", str(HOME / "repo"), "--label", "boundary", "--no-focus")
        item = json.loads(result.stdout)["result"]["root_pane"]
        herdr("agent", "start", "bound" + timeout, "--kind", "pi", "--pane", item["pane_id"],
              "--timeout", timeout, "--", "--wall-coordinator")
        herdr("pane", "read", item["pane_id"], "--source", "detection", "--format", "text")
        herdr("tab", "close", item["tab_id"])
    # A missing launch executable is a real shell failure. Use the supported
    # launch PATH environment argument; no provider or model is invoked.
    empty = HOME / "empty-bin"
    empty.mkdir()
    herdr("agent", "start", "missing", "--kind", "pi", "--pane", pane,
          "--env", "PATH=" + str(empty), "--timeout", "3001")
    herdr("pane", "read", pane, "--source", "visible", "--format", "text")
    herdr("pane", "read", pane, "--source", "detection", "--format", "text")
    herdr("pane", "process-info", "--pane", pane)
    return 0


def transitions():
    _, row = lane()
    pane = row["pane_id"]
    seq = 100
    def state(value):
        nonlocal seq
        seq += 1
        herdr("pane", "report-agent", pane, "--source", "pi", "--agent", "pi",
              "--state", value, "--seq", str(seq))
        herdr("agent", "list")
    for value in ("idle", "working", "blocked", "idle"):
        state(value)
        herdr("agent", "prompt", pane, "-literal unicode 界🦊")
    state("idle")
    waiter = subprocess.Popen(["herdr", "--session", SESSION, "agent", "prompt", pane,
                               "state transition", "--wait", "--until", "working",
                               "--until", "blocked", "--timeout", "3000"],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    time.sleep(.4)
    state("working")
    stdout, stderr = waiter.communicate(timeout=10)
    record(case="idle-to-working-during-prompt-wait", rc=waiter.returncode,
           stdout=stdout, stderr=stderr)
    state("idle")
    state("blocked")
    herdr("agent", "prompt", pane, "blocked-before-submission", "--wait", "--until",
          "working", "--until", "blocked", "--timeout", "0")
    state("working")
    herdr("pane", "process-info", "--pane", pane)
    herdr("server", "restart")
    time.sleep(3)
    herdr("agent", "list")
    herdr("pane", "process-info", "--pane", pane)
    # Closed-tab and closed-pane observations must be explicit errors, not an
    # empty successful screen or a fabricated ready agent.
    herdr("tab", "close", row["tab_id"])
    for args in (("pane", "get", pane), ("pane", "read", pane, "--source", "detection", "--format", "text"),
                 ("pane", "process-info", "--pane", pane), ("agent", "prompt", pane, "closed"),
                 ("tab", "close", row["tab_id"])):
        herdr(*args)
    return 0


if __name__ == "__main__":
    boot()
    try:
        code = {"large-prompt": large_prompt, "empty-notification": empty_notification,
                "transitions": transitions, "boundaries": boundaries,
                "session-selection": session_selection}[sys.argv[1]]()
    finally:
        herdr("session", "stop", SESSION)
        herdr("session", "delete", SESSION)
    sys.exit(code)
