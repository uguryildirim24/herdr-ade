#!/usr/bin/env python3
"""D54-D56: real courier/session and note transports, and notification contract."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import tomllib

HOME = Path.home()
SESSION = "scratch-t-0825"
SOCKET = HOME / ".config/herdr/sessions" / SESSION / "herdr.sock"
LOG = HOME / "runs/herdr-input.jsonl"


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
    # The reset fixture's bashrc restores its default socket; lifecycle hooks
    # and fixture shells must instead stay in this isolated session.
    with (HOME / ".bashrc").open("a") as output:
        output.write(f"\nexport HERDR_SOCKET_PATH={SOCKET}\nexport HERDR_SESSION={SESSION}\n")
    with (HOME / "runs/input-server.log").open("w") as output:
        subprocess.Popen(["herdr", "--session", SESSION, "server"],
                         stdin=subprocess.DEVNULL, stdout=output, stderr=output,
                         start_new_session=True)
    for _ in range(100):
        if subprocess.run(["herdr", "--session", SESSION, "status", "server"],
                          capture_output=True).returncode == 0:
            break
        time.sleep(.1)
    else:
        raise RuntimeError("scratch server did not become ready")
    call("ha", "--version")
    herdr("--version")


def large_prompt():
    (HOME / "control.json").write_text(json.dumps({"hold": ["working"], "delay_ms": 50}))
    if call("python3", str(HOME / "tools/guest.py"), "lane", timeout=150).returncode:
        raise RuntimeError("fixture lane did not start")
    path = HOME / ".herdr-ade/wall/.state/threads/t-0001.toml"
    for _ in range(200):
        row = tomllib.loads(path.read_text())
        if not row.get("prompt_pending", True) and row.get("bootstrap") == "acknowledged":
            break
        time.sleep(.1)
    else:
        raise RuntimeError("fixture skill/brief receipt did not become acknowledged")
    herdr("pane", "report-agent", row["pane_id"], "--source", "pi", "--agent", "pi",
          "--state", "idle", "--seq", "100000")
    note = HOME / "large-note.txt"
    note.write_text("x" * 131072)
    result = call("ha", "thread", "prompt", "wall", row["id"], "--text-file", str(note))
    saved = tomllib.loads(path.read_text())
    notes = saved.get("follow_ups", [])
    summary = [{k: len(v.encode()) if k == "text" else v for k, v in item.items()}
               for item in notes]
    record(record=str(path), follow_ups=summary)
    server = herdr("status", "server")
    delivered = any(item["state"] == "delivered" and item.get("delivered_at")
                    and item["text"] == note.read_text() for item in notes)
    print("EXPECTED: the exact 131072-byte file note receives a delivery receipt; server remains reachable")
    print(f"ACTUAL: rc={result.returncode}, delivered={bool(delivered)}, notes={summary}, server_reachable={server.returncode == 0}")
    return 0 if result.returncode == 0 and delivered and server.returncode == 0 else 1


def empty_notification():
    for title in ("", " ", "\n", "\u2003"):
        result = herdr("notification", "show", title, "--body", "wall contract")
        if result.returncode == 0 or "notification title is empty" not in result.stderr:
            raise RuntimeError("live notification contract changed; inspect evidence")
    print("EXPECTED: the fake rejects the same four blank notification titles as real herdr")
    print("ACTUAL: real returns invalid_params for all four; the host runs the actual fake test next")
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
    print(f"EXPECTED: requested {SESSION} contains {pane}; snapshot={expected}")
    print(f"ACTUAL: courier panes={actual} despite inherited default-session socket")
    return 0 if actual == expected else 1


if __name__ == "__main__":
    try:
        boot()
        code = {"D54": session_selection, "D55": large_prompt,
                "D56": empty_notification}[sys.argv[1]]()
    finally:
        herdr("session", "stop", SESSION)
        herdr("session", "delete", SESSION)
    sys.exit(code)
