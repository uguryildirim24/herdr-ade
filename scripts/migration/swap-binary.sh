#!/bin/sh
# State-dependent swap of ~/.local/bin/herdr (SPEC-ADE §3.3 step 6).
# Usage: swap-binary.sh <source hash> <target hash> <staged binary>
#
# Tests set HERDR_ADE_SWAP_DIR to a throwaway directory so this never writes
# ~/.local/bin. Production leaves that variable unset.

set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <source-sha256> <target-sha256> <staged-binary>" >&2
    exit 2
fi

SOURCE_HASH=$1
TARGET_HASH=$2
STAGED=$3

BIN_DIR=${HERDR_ADE_SWAP_DIR:-"$HOME/.local/bin"}
INSTALLED="$BIN_DIR/herdr"
SHORT=$(printf '%s' "$SOURCE_HASH" | cut -c1-12)
BACKUP="$BIN_DIR/herdr-0.9.0-$SHORT"

sha256_of() {
    # shasum -a 256 prints "<hash>  <path>"; take the hash only.
    shasum -a 256 -- "$1" | awk '{print $1}'
}

mode_of() {
    /usr/bin/stat -f '%Lp' "$1" 2>/dev/null || stat -c '%a' "$1"
}

is_regular() {
    [ -f "$1" ] && [ ! -L "$1" ]
}

fail() {
    echo "swap-binary: $*" >&2
    exit 1
}

[ -n "$SOURCE_HASH" ] || fail "source hash is empty"
[ -n "$TARGET_HASH" ] || fail "target hash is empty"
[ -f "$STAGED" ] || fail "staged binary is not a file: $STAGED"

mkdir -p "$BIN_DIR"

if ! is_regular "$INSTALLED"; then
    fail "installed path is missing, a symlink, or not a regular file: $INSTALLED"
fi

INSTALLED_HASH=$(sha256_of "$INSTALLED")

if [ "$INSTALLED_HASH" = "$TARGET_HASH" ]; then
    # Already swapped. Backup must still hold the source bytes; write nothing.
    if ! is_regular "$BACKUP"; then
        fail "install already at target, but backup is missing or not a regular file: $BACKUP"
    fi
    BACKUP_HASH=$(sha256_of "$BACKUP")
    if [ "$BACKUP_HASH" != "$SOURCE_HASH" ]; then
        fail "install already at target, but backup hash is $BACKUP_HASH, not source"
    fi
    echo "swap-binary: already installed (target); backup reused; wrote nothing"
    exit 0
fi

if [ "$INSTALLED_HASH" != "$SOURCE_HASH" ]; then
    fail "installed hash is $INSTALLED_HASH; expected source $SOURCE_HASH or target $TARGET_HASH"
fi

# Backup branch.
if [ -e "$BACKUP" ]; then
    if ! is_regular "$BACKUP"; then
        fail "backup exists but is not a regular file: $BACKUP"
    fi
    BACKUP_HASH=$(sha256_of "$BACKUP")
    if [ "$BACKUP_HASH" = "$SOURCE_HASH" ]; then
        echo "swap-binary: backup already holds source; reuse, no write"
    else
        fail "backup exists with hash $BACKUP_HASH, not source"
    fi
else
    # Exclusive-create: noclobber fails if the path appeared meanwhile.
    # cp ignores noclobber; redirection does not (SPEC-ADE §3.3 step 6).
    ( set -o noclobber && cat -- "$INSTALLED" > "$BACKUP" ) || fail "exclusive create of backup failed: $BACKUP"
    chmod "$(mode_of "$INSTALLED")" "$BACKUP" 2>/dev/null || chmod 755 "$BACKUP"
    BACKUP_HASH=$(sha256_of "$BACKUP")
    if [ "$BACKUP_HASH" != "$SOURCE_HASH" ]; then
        fail "backup hash is $BACKUP_HASH after create, not source"
    fi
    echo "swap-binary: backup created $BACKUP"
fi

STAGE=$(mktemp "$BIN_DIR/herdr.XXXXXX") || fail "mktemp failed"
trap 'rm -f -- "$STAGE"' EXIT
cat -- "$STAGED" > "$STAGE"
STAGE_HASH=$(sha256_of "$STAGE")
if [ "$STAGE_HASH" != "$TARGET_HASH" ]; then
    fail "staged copy hash is $STAGE_HASH, not target"
fi
chmod "$(mode_of "$INSTALLED")" "$STAGE" 2>/dev/null || chmod 755 "$STAGE"

# Same filesystem rename; the path is never missing.
mv -f -- "$STAGE" "$INSTALLED"
trap - EXIT

FINAL=$(sha256_of "$INSTALLED")
if [ "$FINAL" != "$TARGET_HASH" ]; then
    fail "after mv, installed hash is $FINAL, not target"
fi

echo "swap-binary: installed target at $INSTALLED"
exit 0
