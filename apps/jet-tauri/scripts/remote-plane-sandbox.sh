#!/usr/bin/env bash
# Runs a command next to a disposable remote Plane, for the Wave 3.1 manual
# end-to-end matrix (docs/wave-3.1.md, "Verification").
#
#   scripts/remote-plane-sandbox.sh <path-to-jetd> -- <command> [args...]
#
# Everything lives in one directory under $XDG_RUNTIME_DIR and is deleted on
# exit. Nothing in ~/.ssh, ~/.jet or the login keyring is read or written:
#
# - an unprivileged sshd on 127.0.0.1 with its own host key, accepting only a
#   sandbox client key and only the fixed remote command `jetd connect --stdio`;
# - an `ssh` wrapper first on PATH that adds `-F <sandbox ssh_config>`, so the
#   app's fixed ssh options still apply, host keys are checked against a
#   sandbox known_hosts file, and every spawn is appended to a log;
# - `jetd serve` for the remote Plane and for the enrolling side's local Plane;
# - a private D-Bus session (no service activation) with its own GNOME
#   Keyring (password "e2e"), and no display, so an unlock prompt can
#   neither appear on screen nor be answered.
#
# The command sees JET_E2E_* variables describing the sandbox, and
# $JET_E2E_CTL {stop|start}-{remote|keyring} and lock-keyring to change it.
set -euo pipefail

if [[ $# -lt 3 || $2 != "--" ]]; then
	echo "usage: $0 <path-to-jetd> -- <command> [args...]" >&2
	exit 64
fi
JETD=$(realpath "$1")
shift 2
[[ -x $JETD ]] || { echo "not executable: $JETD" >&2; exit 66; }

# Unix socket paths are limited to 108 bytes, so keep the root short.
ROOT=$(mktemp -d "${XDG_RUNTIME_DIR:?}/jet-e2e.XXXXXX")
chmod 700 "$ROOT"
cleanup() {
	for file in "$ROOT"/run/*.pid; do
		[[ -f $file ]] && kill "$(cat "$file")" 2>/dev/null || true
	done
	wait 2>/dev/null || true
	rm -rf "$ROOT"
}
trap cleanup EXIT

mkdir -p "$ROOT"/{ssh,bin,run,remote/.jet,local/.jet,xdg-data,keyring}
chmod 700 "$ROOT/keyring"

# sshd ----------------------------------------------------------------------
PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
ssh-keygen -q -t ed25519 -N '' -C jet-e2e-host -f "$ROOT/ssh/host_key"
ssh-keygen -q -t ed25519 -N '' -C jet-e2e-client -f "$ROOT/ssh/client_key"
cp "$ROOT/ssh/client_key.pub" "$ROOT/ssh/authorized_keys"
printf '[127.0.0.1]:%s %s\n' "$PORT" "$(cut -d' ' -f1,2 "$ROOT/ssh/host_key.pub")" \
	>"$ROOT/ssh/known_hosts"
cp "$ROOT/ssh/known_hosts" "$ROOT/ssh/known_hosts.trusted"

cat >"$ROOT/bin/remote-command" <<EOF
#!/bin/sh
# Only the app's fixed remote command is accepted.
[ "\$SSH_ORIGINAL_COMMAND" = "jetd connect --stdio" ] || exit 126
exec "$JETD" connect --stdio --home "$ROOT/remote/.jet"
EOF
chmod 755 "$ROOT/bin/remote-command"

cat >"$ROOT/ssh/sshd_config" <<EOF
ListenAddress 127.0.0.1
Port $PORT
HostKey $ROOT/ssh/host_key
PidFile $ROOT/run/sshd.pid
AuthorizedKeysFile $ROOT/ssh/authorized_keys
AllowUsers $(id -un)
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
StrictModes no
PermitTTY no
AllowTcpForwarding no
AllowAgentForwarding no
X11Forwarding no
ForceCommand $ROOT/bin/remote-command
EOF
/usr/bin/sshd -f "$ROOT/ssh/sshd_config" -E "$ROOT/run/sshd.log"

cat >"$ROOT/ssh/ssh_config" <<EOF
Host jet-e2e
	HostName 127.0.0.1
	Port $PORT
	User $(id -un)
	IdentityFile $ROOT/ssh/client_key
	IdentitiesOnly yes
	IdentityAgent none
	UserKnownHostsFile $ROOT/ssh/known_hosts
	GlobalKnownHostsFile /dev/null
EOF

cat >"$ROOT/bin/ssh" <<EOF
#!/bin/sh
# One line per spawn: time, parent command name, arguments.
printf '%s %s %s\n' "\$(date +%s%N)" "\$(ps -o comm= -p \$PPID)" "\$*" >>"$ROOT/run/ssh-spawns.log"
exec /usr/bin/ssh -F "$ROOT/ssh/ssh_config" "\$@"
EOF
chmod 755 "$ROOT/bin/ssh"
: >"$ROOT/run/ssh-spawns.log"

# Planes ----------------------------------------------------------------------
start_jetd() { # name
	"$JETD" serve --home "$ROOT/$1/.jet" >>"$ROOT/run/jetd-$1.log" 2>&1 &
	echo $! >"$ROOT/run/jetd-$1.pid"
	for _ in $(seq 100); do
		[[ -S $ROOT/$1/.jet/runtime/jetd.sock ]] && return 0
		sleep 0.1
	done
	echo "jetd ($1) did not start; see $ROOT/run/jetd-$1.log" >&2
	return 1
}
stop_jetd() { # name
	local pid
	pid=$(cat "$ROOT/run/jetd-$1.pid")
	kill "$pid" 2>/dev/null || true
	while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done
	rm -f "$ROOT/run/jetd-$1.pid"
}
start_jetd remote
start_jetd local

# Control helper for the command -----------------------------------------------
cat >"$ROOT/bin/ctl" <<EOF
#!/usr/bin/env bash
set -euo pipefail
ROOT="$ROOT"; JETD="$JETD"
$(declare -f start_jetd stop_jetd)
case "\$1" in
	stop-remote) stop_jetd remote ;;
	start-remote) start_jetd remote ;;
	stop-keyring) kill "\$(cat "\$ROOT/run/keyring.pid")"; rm -f "\$ROOT/run/keyring.pid"; sleep 0.5 ;;
	start-keyring)
		printf e2e | gnome-keyring-daemon --unlock --daemonize --components=secrets \\
			--control-directory="\$ROOT/keyring" >/dev/null
		pgrep -n -f -- "--control-directory=\$ROOT/keyring" >"\$ROOT/run/keyring.pid" ;;
	lock-keyring) secret-tool lock --collection=/org/freedesktop/secrets/collection/login ;;
	*) echo "unknown: \$1" >&2; exit 64 ;;
esac
EOF
chmod 755 "$ROOT/bin/ctl"

export JET_E2E_ROOT=$ROOT
export JET_E2E_DESTINATION=jet-e2e
export JET_E2E_LOCAL_HOME=$ROOT/local
export JET_E2E_REMOTE_HOME=$ROOT/remote
export JET_E2E_SPAWN_LOG=$ROOT/run/ssh-spawns.log
export JET_E2E_KNOWN_HOSTS=$ROOT/ssh/known_hosts
export JET_E2E_CTL=$ROOT/bin/ctl
export PATH=$ROOT/bin:$PATH
export XDG_DATA_HOME=$ROOT/xdg-data

# The keyring and the command share a private session bus with no service
# activation, so a stopped keyring stays stopped and no prompter is started.
# No display either: an unlock prompt cannot appear on the desktop.
cat >"$ROOT/run/session.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=$ROOT/run</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF
dbus-run-session --config-file="$ROOT/run/session.conf" -- env -u DISPLAY -u WAYLAND_DISPLAY bash -c '
	"$JET_E2E_CTL" start-keyring
	status=0
	"$@" || status=$?
	[[ -f $JET_E2E_ROOT/run/keyring.pid ]] && kill "$(cat "$JET_E2E_ROOT/run/keyring.pid")" 2>/dev/null
	exit $status
' bash "$@"
