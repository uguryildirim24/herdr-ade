#!/bin/sh
# Guard check against the isolated pi (SPEC-pi v2 §7, row T7; §3.7).
#
# Runs the shipping `extensions/herdr-pi-guard.ts` inside the research pi
# 0.85.1 with a mock provider that always answers 429 or 401. Every file
# lives under /var/tmp/ade-a5/; ~/.pi is never read or written and no login
# is used. A fake `ha` records the `ha waiting` line the guard runs.
#
# Usage: sh src/pi/testdata/guard-check.sh
set -eu

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH= cd -- "$HERE/../../.." && pwd)
ROOT=${ROOT:-/var/tmp/ade-a5/guard-test}
PI_JS=${PI_JS:-/var/tmp/pi-research-a/npm/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js}
NODE=${NODE:-node}
PORT=${PORT:-19890}

if [ ! -f "$PI_JS" ]; then
  echo "SKIP: isolated pi not found at $PI_JS"
  exit 0
fi

rm -rf "$ROOT"
mkdir -p "$ROOT/agent/extensions" "$ROOT/bin" "$ROOT/log"
cp "$REPO/extensions/herdr-pi-guard.ts" "$ROOT/agent/extensions/herdr-pi-guard.ts"

cat > "$ROOT/agent/settings.json" <<'JSON'
{
  "defaultProjectTrust": "never",
  "enableInstallTelemetry": false,
  "quietStartup": true,
  "skills": { "enabled": false },
  "retry": { "enabled": true, "maxRetries": 1, "provider": { "maxRetries": 0, "maxRetryDelayMs": 60000 } }
}
JSON

cat > "$ROOT/agent/models.json" <<JSON
{
  "providers": {
    "mock-provider": {
      "baseUrl": "http://127.0.0.1:$PORT/v1",
      "api": "openai-completions",
      "apiKey": "mock-key",
      "models": [
        { "id": "mock-model", "name": "Mock Model", "input": ["text"], "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 } }
      ]
    }
  }
}
JSON

cat > "$ROOT/bin/ha" <<'SH'
#!/bin/sh
printf '%s\n' "$*" >> "$HERDR_ADE_GUARD_LOG"
SH
chmod +x "$ROOT/bin/ha"

# A fake herdr for the parent fallback. Every case points HERDR_BIN_PATH at
# it, so the guard can never reach a real herdr server from this check.
cat > "$ROOT/bin/herdr" <<'SH'
#!/bin/sh
if [ "$1 $2" = "agent list" ]; then
  printf '%s\n' '{"result":{"agents":[{"name":"guard-lane","pane_id":"wG:p9","tokens":{"parent":"wG:p1"}}]}}'
  exit 0
fi
printf '%s\n' "$*" >> "$HERDR_ADE_GUARD_LOG"
SH
chmod +x "$ROOT/bin/herdr"

fail=0
say() {
  printf '%s\n' "$1"
}

# $1 mode, $2 label, $3 expected class, $4 expected mock hits (pi's v2
# retry cap: 2 for a 429, 1 for a 401), $5 `ade` (HERDR_ADE_LAUNCH set, a
# fake `ha`) or `parent` (no HERDR_ADE_LAUNCH, the herdr parent fallback)
run_case() {
  mode=$1
  label=$2
  class=$3
  want_hits=$4
  route=$5
  hits_file="$ROOT/log/hits-$mode-$route.log"
  guard_log="$ROOT/log/ha-$mode-$route.log"
  : > "$hits_file"
  : > "$guard_log"

  "$NODE" "$HERE/mock-provider.js" "$mode" "$PORT" "$hits_file" >"$ROOT/log/mock-$mode.out" 2>&1 &
  mock_pid=$!
  sleep 1

  if [ "$route" = ade ]; then
    PIPATH="$ROOT/bin:$PATH"
    launch="guard/lane/1/abc"
    want="^waiting mock-provider $class: "
  else
    # No `ha` on this PATH: only the fake herdr.
    mkdir -p "$ROOT/herdr-only"
    ln -sf "$ROOT/bin/herdr" "$ROOT/herdr-only/herdr"
    PIPATH="$ROOT/herdr-only:$PATH"
    launch=""
    want="^agent prompt wG:p1 WAITING guard-lane mock-provider $class: "
  fi
  env -u HERDR_SOCKET_PATH -u HERDR_SESSION \
    HERDR_ADE_GUARD_LOG="$guard_log" \
    HERDR_ADE_LAUNCH="$launch" \
    HERDR_PANE_ID="wG:p9" \
    HERDR_BIN_PATH="$ROOT/bin/herdr" \
    PI_CODING_AGENT_DIR="$ROOT/agent" \
    PATH="$PIPATH" \
    "$NODE" "$PI_JS" --provider mock-provider --model mock-model -p "hello" --no-skills \
    >"$ROOT/log/pi-$mode-$route.out" 2>&1 </dev/null || true
  kill "$mock_pid" 2>/dev/null || true
  wait "$mock_pid" 2>/dev/null || true

  hits=$(wc -l < "$hits_file" | tr -d ' ')
  waits=$(grep -c "$want" "$guard_log" 2>/dev/null || true)
  lines=$(wc -l < "$guard_log" | tr -d ' ')
  dones=$(grep -ci "done" "$guard_log" 2>/dev/null || true)
  if [ "$hits" -eq "$want_hits" ] && [ "$waits" -eq 1 ] && [ "$lines" -eq 1 ] && [ "$dones" -eq 0 ]; then
    say "PASS $label: mock hits $hits, one WAITING (class $class, $route), no done"
    sed 's/^/  /' "$guard_log"
  else
    say "FAIL $label: mock hits $hits (want $want_hits), matching lines $waits, lines $lines, done lines $dones"
    sed 's/^/  log: /' "$guard_log"
    say "  --- pi output ---"
    sed 's/^/  /' "$ROOT/log/pi-$mode-$route.out" | tail -30
    fail=1
  fi
}

run_case limit "T7a 429 becomes blocked + WAITING limit" limit 2 ade
run_case login "T7b 401 becomes blocked + WAITING login" login 1 ade
run_case limit "T7c 429 without ADE: WAITING to the parent" limit 2 parent

if [ "$fail" -eq 0 ]; then
  say "GUARD PASS"
else
  say "GUARD FAIL"
  exit 1
fi
