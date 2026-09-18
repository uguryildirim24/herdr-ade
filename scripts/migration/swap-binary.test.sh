#!/bin/sh
# macOS tests for swap-binary.sh. Never writes ~/.local/bin.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
SCRIPT="$ROOT/scripts/migration/swap-binary.sh"
chmod +x "$SCRIPT"

WORKDIR=$(mktemp -d "${TMPDIR:-/tmp}/ade-swap.XXXXXX")
cleanup() { rm -rf -- "$WORKDIR"; }
trap cleanup EXIT

SWAP="$WORKDIR/bin"
export HERDR_ADE_SWAP_DIR="$SWAP"
mkdir -p "$SWAP"

hash_of() { shasum -a 256 -- "$1" | awk '{print $1}'; }

printf 'SOURCE-BYTES\n' > "$WORKDIR/source"
printf 'TARGET-BYTES\n' > "$WORKDIR/target"
SOURCE_HASH=$(hash_of "$WORKDIR/source")
TARGET_HASH=$(hash_of "$WORKDIR/target")
SHORT=$(printf '%s' "$SOURCE_HASH" | cut -c1-12)
BACKUP="$SWAP/herdr-0.9.0-$SHORT"

fail_case() {
    name=$1
    shift
    if "$SCRIPT" "$@" >"$WORKDIR/out" 2>"$WORKDIR/err"; then
        echo "FAIL $name: expected non-zero" >&2
        cat "$WORKDIR/err" >&2
        exit 1
    fi
    echo "ok   $name (refused)"
}

# First run: copy source to backup, replace with target.
cp "$WORKDIR/source" "$SWAP/herdr"
chmod 755 "$SWAP/herdr"
"$SCRIPT" "$SOURCE_HASH" "$TARGET_HASH" "$WORKDIR/target"
[ "$(hash_of "$SWAP/herdr")" = "$TARGET_HASH" ] || { echo "FAIL first run: installed hash"; exit 1; }
[ -f "$BACKUP" ] || { echo "FAIL first run: backup missing"; exit 1; }
[ "$(hash_of "$BACKUP")" = "$SOURCE_HASH" ] || { echo "FAIL first run: backup hash"; exit 1; }
echo "ok   first run"

# Idempotent: already target, backup is source → write nothing.
BEFORE=$(/usr/bin/stat -f '%m' "$SWAP/herdr")
sleep 1
"$SCRIPT" "$SOURCE_HASH" "$TARGET_HASH" "$WORKDIR/target"
AFTER=$(/usr/bin/stat -f '%m' "$SWAP/herdr")
[ "$BEFORE" = "$AFTER" ] || { echo "FAIL idempotent: mtime changed"; exit 1; }
[ "$(hash_of "$SWAP/herdr")" = "$TARGET_HASH" ] || { echo "FAIL idempotent: installed hash"; exit 1; }
echo "ok   already-target with source backup (no write)"

# Already target, backup missing → fail.
rm -f "$BACKUP"
fail_case "already-target missing backup" "$SOURCE_HASH" "$TARGET_HASH" "$WORKDIR/target"

# Restore backup for the next cases.
cp "$WORKDIR/source" "$BACKUP"
cp "$WORKDIR/target" "$SWAP/herdr"

# Installed hash matches neither source nor target → fail.
printf 'OTHER-BYTES\n' > "$SWAP/herdr"
fail_case "installed hash matches neither" "$SOURCE_HASH" "$TARGET_HASH" "$WORKDIR/target"

# Reset to source; backup exists with wrong hash → fail.
cp "$WORKDIR/source" "$SWAP/herdr"
printf 'WRONG-BACKUP\n' > "$BACKUP"
fail_case "backup wrong hash" "$SOURCE_HASH" "$TARGET_HASH" "$WORKDIR/target"

# Backup exists with source; installed is source → reuse backup, no rewrite of backup.
cp "$WORKDIR/source" "$BACKUP"
BACKUP_MTIME=$(/usr/bin/stat -f '%m' "$BACKUP")
sleep 1
"$SCRIPT" "$SOURCE_HASH" "$TARGET_HASH" "$WORKDIR/target"
[ "$(hash_of "$SWAP/herdr")" = "$TARGET_HASH" ] || { echo "FAIL reuse backup: installed"; exit 1; }
[ "$(/usr/bin/stat -f '%m' "$BACKUP")" = "$BACKUP_MTIME" ] || { echo "FAIL reuse backup: backup rewritten"; exit 1; }
echo "ok   backup already source is reused"

# Symlink at installed → fail.
rm -f "$SWAP/herdr"
ln -s "$WORKDIR/source" "$SWAP/herdr"
fail_case "installed is a symlink" "$SOURCE_HASH" "$TARGET_HASH" "$WORKDIR/target"

echo "all swap-binary tests passed"
