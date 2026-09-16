#!/bin/bash
# What a sandboxed plugin can actually reach — the boundary `sdk/src/sandbox.ts` builds, checked
# against a real kernel rather than asserted.
#
# `sandbox.test.ts` pins the argv; this pins what that argv DOES: no admin token, no ~/.ssh, no
# host process in /proc, no network, and exactly the declared path, read-only. Keep the flags
# below in step with `bwrapArgv`.
#
#   docker run --rm --privileged -v $PWD/scripts/check-plugin-sandbox.sh:/check.sh:ro \
#     ubuntu:24.04 bash /check.sh
#
# Needs bubblewrap and unprivileged user namespaces, so it runs on Linux only.
set -u
apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq bubblewrap >/dev/null 2>&1 || { echo "FAIL: no bubblewrap"; exit 1; }

HOME_DIR=/root
mkdir -p "$HOME_DIR/.config/punktfunk" "$HOME_DIR/.ssh" "$HOME_DIR/.local/share/Steam"
echo "PUNKTFUNK_MGMT_TOKEN=supersecret" > "$HOME_DIR/.config/punktfunk/mgmt-token"
echo "private key" > "$HOME_DIR/.ssh/id_ed25519"
echo "steam data" > "$HOME_DIR/.local/share/Steam/marker"
mkdir -p /run/punktfunk-state && echo "state" > /run/punktfunk-state/marker

# The same flags sdk/src/sandbox.ts emits, for a manifest declaring ~/.local/share/Steam.
ARGV=(
  --unshare-all --die-with-parent --new-session --clearenv
  --proc /proc --dev /dev --tmpfs /tmp
  --ro-bind /usr /usr
  --symlink usr/lib /lib --symlink usr/lib64 /lib64 --symlink usr/bin /bin --symlink usr/sbin /sbin
  --bind /run/punktfunk-state /run/punktfunk/plugin-state
  --ro-bind-try "$HOME_DIR/.local/share/Steam" "$HOME_DIR/.local/share/Steam"
)

run_in() { bwrap "${ARGV[@]}" /bin/sh -c "$1" 2>&1; }

pass=0; fail=0
check() { # name, command, expectation: "empty" | "nonempty"
  out="$(run_in "$2")"
  if [ "$3" = empty ] && [ -z "$out" ]; then echo "  ok   $1"; pass=$((pass+1));
  elif [ "$3" = nonempty ] && [ -n "$out" ]; then echo "  ok   $1"; pass=$((pass+1));
  else echo "  FAIL $1 -> '$out'"; fail=$((fail+1)); fi
}

echo "== what a plugin can reach"
check "the admin token is not there"        "cat $HOME_DIR/.config/punktfunk/mgmt-token 2>/dev/null" empty
check "~/.ssh is not there"                 "cat $HOME_DIR/.ssh/id_ed25519 2>/dev/null" empty
check "the declared Steam root IS there"    "cat $HOME_DIR/.local/share/Steam/marker 2>/dev/null" nonempty
check "its own state dir IS writable"       "echo x > /run/punktfunk/plugin-state/w && cat /run/punktfunk/plugin-state/w" nonempty
check "the declared root is READ-ONLY"      "{ echo x > $HOME_DIR/.local/share/Steam/w && echo wrote; } 2>/dev/null" empty

echo "== what a plugin can see of the host"
# The host's own process, running outside: it must not appear in /proc at all.
sleep 300 &
HOST_PID=$!
check "the host process is not in /proc"    "ls /proc/$HOST_PID 2>/dev/null" empty
check "it cannot be signalled"              "kill -0 $HOST_PID 2>/dev/null && echo reachable" empty
check "/proc/1 is the sandbox, not the host" "readlink /proc/1/exe | grep -q sleep && echo host" empty
check "no network"                          "cat /proc/net/tcp 2>/dev/null | tail -n +2 | grep -q . && echo sockets" empty
kill $HOST_PID 2>/dev/null

echo
echo "passed=$pass failed=$fail"
[ "$fail" -eq 0 ]
