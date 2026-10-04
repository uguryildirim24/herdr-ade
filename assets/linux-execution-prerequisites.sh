# ADE execution prerequisites: trusted coordinator install only, never a lane.
# Do not relax global user-namespace policy or handle credentials.
case "$(uname -s)" in
  Linux) ;;
  *) printf '%s\n' 'advisory: non-Linux machine; Linux prerequisites not applicable'; exit 0 ;;
esac
export ADE_BWRAP=__BWRAP__ ADE_PROFILE=__PROFILE__
export PATH="$PATH:/usr/sbin:/sbin"
if sh -eu <<'ADE_PREREQUISITES'
package() {
  if command -v apt-get >/dev/null 2>&1; then
    sudo -n env DEBIAN_FRONTEND=noninteractive apt-get update -qq
    sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install -y "$1"
  elif command -v dnf >/dev/null 2>&1; then
    sudo -n dnf install -y "$1"
  else
    echo 'no supported package manager (apt-get/dnf)' >&2
    exit 1
  fi
}
if [ ! -x "$ADE_BWRAP" ]; then
  package bubblewrap
  test -x "$ADE_BWRAP"
  echo 'bubblewrap installed'
else
  echo 'bubblewrap already installed'
fi
policy=$(mktemp)
trap 'rm -f "$policy"' EXIT HUP INT TERM
cat > "$policy" <<'ADE_APPARMOR'
abi <abi/4.0>,
include <tunables/global>
profile ade_bwrap /usr/bin/bwrap flags=(unconfined) {
  userns,
}
ADE_APPARMOR
sudo -n install -d -m 0755 "$(dirname "$ADE_PROFILE")"
if sudo -n cmp -s "$policy" "$ADE_PROFILE"; then
  echo 'AppArmor profile already installed'
else
  sudo -n install -m 0644 "$policy" "$ADE_PROFILE"
  echo 'AppArmor profile installed'
fi
if [ -r /sys/module/apparmor/parameters/enabled ] && grep -q '^Y' /sys/module/apparmor/parameters/enabled; then
  command -v apparmor_parser >/dev/null 2>&1 || package apparmor
  sudo -n "$(command -v apparmor_parser)" -r "$ADE_PROFILE"
  echo 'AppArmor profile loaded; global restriction unchanged'
else
  echo 'AppArmor inactive; profile ready for next boot; global policy unchanged'
fi
ADE_PREREQUISITES
then provision=ok
else provision=failed
fi
probe_log=$(mktemp)
trap 'rm -f "$probe_log"' EXIT HUP INT TERM
if __PROBE__ > "$probe_log" 2>&1; then
  printf 'bounded: provisioning=%s; doctor namespace probe passed\n' "$provision"
else
  printf 'advisory: provisioning=%s; doctor namespace probe failed; new lanes keep working in advisory mode\n' "$provision"
  cat "$probe_log"
fi
