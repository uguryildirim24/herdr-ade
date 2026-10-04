# Additional wall faults

Add an executable here named for the fault. `wall --instance N fault NAME ARG...`
dispatches to `tools/wall/faults/NAME --instance N ARG...` (and `--box` when set).
The host controller applies `--when` and `--after` before dispatch. Default is
passed as `--instance 0`; numbered instances are 1..8.

Import `Instance` from `tools/wall/instance.py` for all names and paths. Delegate
guest operations through `wall --instance N enter [--box] COMMAND` (omit the
flag for zero), never arbitrary host PIDs or paths. Fault executables are
privileged host tools: review their scope before running, as with the built-in
faults. Do not put generated or guest-controlled executables here. Install
copies these tools into each instance; rerun install to refresh guest helpers.

Example host invocation:

```bash
sudo tools/wall/wall --instance 3 fault YOUR-FAULT argument --after 2
```
