#!/bin/sh
# zymon-client installer (Linux: Raspberry Pi and other ARM hosts, macOS: Apple Silicon).
#
#   curl -fsSL https://raw.githubusercontent.com/jeroenflvr/zymon/main/zymon/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --server xymon.example.org
#
# Detects OS/arch, downloads the matching static binary from the GitHub release, verifies its SHA-256,
# installs it, writes the config with `zymon-client init` (asking for the Xymon server) and sets up the
# service (systemd / launchd). Re-running upgrades the binary and keeps the existing config.
#
# Options (or the environment variable in brackets):
#   --server HOST[:PORT]  Xymon server, repeatable          [ZYMON_SERVER, space separated]
#   --hostname NAME       host name as known to the server  [ZYMON_HOSTNAME]
#   --version TAG         release tag, e.g. v0.1.0          [ZYMON_VERSION, default: latest]
#   --prefix DIR          binary goes to DIR/bin            [ZYMON_PREFIX, default /usr/local]
#   --config PATH         config file                       [ZYMON_CONFIG]
#   --no-service          do not install or start a service
#   --yes                 never prompt; use defaults        [ZYMON_YES=1]
#   --uninstall           remove service and binary (keeps the config; add --purge to remove it too)
# Other environment: ZYMON_STATE_DIR (default /var/lib/zymon), ZYMON_REPO (default jeroenflvr/zymon), ZYMON_BASE_URL (download directory, may be file://).

set -eu

REPO=${ZYMON_REPO:-jeroenflvr/zymon}
VERSION=${ZYMON_VERSION:-latest}
PREFIX=${ZYMON_PREFIX:-/usr/local}
CONFIG=${ZYMON_CONFIG:-}
HOSTNAME_ARG=${ZYMON_HOSTNAME:-}
SERVERS=${ZYMON_SERVER:-}
YES=${ZYMON_YES:-0}
SERVICE=1
UNINSTALL=0
PURGE=0

say() { printf '%s\n' "$*"; }
step() { printf '\n==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() { sed -n '2,/^$/p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//' || true; }

while [ $# -gt 0 ]; do
	case $1 in
	--server) [ $# -ge 2 ] || die "--server needs a value"; SERVERS="$SERVERS $2"; shift ;;
	--hostname) [ $# -ge 2 ] || die "--hostname needs a value"; HOSTNAME_ARG=$2; shift ;;
	--version) [ $# -ge 2 ] || die "--version needs a value"; VERSION=$2; shift ;;
	--prefix) [ $# -ge 2 ] || die "--prefix needs a value"; PREFIX=$2; shift ;;
	--config) [ $# -ge 2 ] || die "--config needs a value"; CONFIG=$2; shift ;;
	--no-service) SERVICE=0 ;;
	--yes | -y) YES=1 ;;
	--uninstall) UNINSTALL=1 ;;
	--purge) PURGE=1 ;;
	-h | --help) usage; exit 0 ;;
	*) die "unknown option: $1 (try --help)" ;;
	esac
	shift
done

# ---- detect the platform ----------------------------------------------------------------------------------
OS=$(uname -s)
ARCH=$(uname -m)
case "$OS/$ARCH" in
Linux/armv6l) LABEL=linux-armv6 ;;
Linux/armv7l | Linux/armv8l) LABEL=linux-armv7 ;; # armv8l = 32-bit userland on a 64-bit kernel
Linux/aarch64 | Linux/arm64) LABEL=linux-aarch64 ;;
Darwin/arm64) LABEL=darwin-arm64 ;;
*) die "no zymon-client build for $OS/$ARCH (supported: Linux armv6l/armv7l/aarch64, macOS arm64)" ;;
esac

BINDIR=$PREFIX/bin
BIN=$BINDIR/zymon-client
if [ "$OS" = Darwin ]; then
	[ -n "$CONFIG" ] || CONFIG=/usr/local/etc/zymon/client.toml
	STATE_DIR=${ZYMON_STATE_DIR:-/var/lib/zymon}
	PLIST=/Library/LaunchDaemons/org.zymon.client.plist
	LOGFILE=/usr/local/var/log/zymon-client.log
else
	[ -n "$CONFIG" ] || CONFIG=/etc/zymon/client.toml
	STATE_DIR=${ZYMON_STATE_DIR:-/var/lib/zymon}
	UNIT=/etc/systemd/system/zymon-client.service
fi
CONFDIR=$(dirname "$CONFIG")

# ---- privileges and helpers ---------------------------------------------------------------------------------
ME=$(id -un)
if [ "$(id -u)" = 0 ]; then
	SUDO=""
elif command -v sudo >/dev/null 2>&1; then
	SUDO="sudo"
else
	SUDO="none"
fi

# priv CMD...: run with root rights when we are not root (only fails if sudo is missing and it is needed).
priv() {
	if [ -z "$SUDO" ]; then
		"$@"
	elif [ "$SUDO" = none ]; then
		die "need root for: $* (run as root or install sudo)"
	else
		sudo "$@"
	fi
}

# True when we can write to path (or, if it does not exist yet, to its nearest existing parent).
writable() {
	p=$1
	while [ ! -e "$p" ]; do p=$(dirname "$p"); done
	[ -w "$p" ]
}

# A user-level install (no service, directories we own) needs no root at all.
relax_sudo() {
	if [ -n "$SUDO" ] && [ "$SERVICE" = 0 ] && writable "$PREFIX/bin" && writable "$CONFDIR" && writable "$STATE_DIR"; then
		SUDO=""
	fi
}
relax_sudo

# Ask on the terminal even when the script itself arrives through a pipe (curl | sh).
have_tty() { [ "$YES" != 1 ] && [ -r /dev/tty ] && [ -w /dev/tty ]; }

ask() { # ask "question" "default" -> $REPLY
	if have_tty; then
		if [ -n "$2" ]; then printf '%s [%s]: ' "$1" "$2" >/dev/tty; else printf '%s: ' "$1" >/dev/tty; fi
		IFS= read -r REPLY </dev/tty || REPLY=""
		[ -n "$REPLY" ] || REPLY=$2
	else
		REPLY=$2
	fi
}

confirm() { # confirm "question" default(y/n)
	if ! have_tty; then [ "$2" = y ]; return; fi
	ask "$1 (y/n)" "$2"
	case $REPLY in y | Y | yes | YES) return 0 ;; *) return 1 ;; esac
}

sq() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }

# as_user USER CMD...: run CMD as USER.
as_user() {
	u=$1
	shift
	if [ "$u" = "$ME" ]; then
		"$@"
	elif [ -z "$SUDO" ]; then
		if command -v runuser >/dev/null 2>&1; then
			runuser -u "$u" -- "$@"
		else
			cmd=""
			for a in "$@"; do cmd="$cmd $(sq "$a")"; done
			su -s /bin/sh "$u" -c "$cmd"
		fi
	elif [ "$SUDO" = none ]; then
		die "cannot switch to user $u without root or sudo"
	else
		sudo -u "$u" "$@"
	fi
}

# ---- uninstall -----------------------------------------------------------------------------------------------
if [ "$UNINSTALL" = 1 ]; then
	step "Uninstalling zymon-client"
	if [ "$OS" = Darwin ]; then SVCFILE=$PLIST; else SVCFILE=$UNIT; fi
	[ -f "$SVCFILE" ] || { SERVICE=0; relax_sudo; }
	if [ "$OS" = Darwin ]; then
		if [ -f "$PLIST" ]; then
			priv launchctl bootout system "$PLIST" 2>/dev/null || true
			priv rm -f "$PLIST"
		fi
	elif command -v systemctl >/dev/null 2>&1 && [ -f "$UNIT" ]; then
		priv systemctl disable --now zymon-client 2>/dev/null || true
		priv rm -f "$UNIT"
		priv systemctl daemon-reload 2>/dev/null || true
	fi
	priv rm -f "$BIN"
	if [ "$PURGE" = 1 ]; then
		priv rm -rf "$CONFDIR" "$STATE_DIR"
		say "Removed binary, service, config and state."
	else
		say "Removed binary and service. Config kept in $CONFDIR (use --purge to delete it)."
	fi
	exit 0
fi

# ---- download ------------------------------------------------------------------------------------------------
if command -v curl >/dev/null 2>&1; then
	fetch() { curl -fsSL --retry 3 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
	fetch() { wget -q -O "$2" "$1"; }
else
	die "need curl or wget"
fi

if command -v sha256sum >/dev/null 2>&1; then
	sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
	sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
	die "need sha256sum or shasum to verify the download"
fi

if [ -n "${ZYMON_BASE_URL:-}" ]; then
	BASE=$ZYMON_BASE_URL
elif [ "$VERSION" = latest ]; then
	BASE=https://github.com/$REPO/releases/latest/download
else
	BASE=https://github.com/$REPO/releases/download/$VERSION
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM

step "Detected $OS/$ARCH, installing build $LABEL ($VERSION)"
ASSET=zymon-client-$LABEL
fetch "$BASE/$ASSET" "$TMP/$ASSET" || die "download failed: $BASE/$ASSET"
fetch "$BASE/SHA256SUMS" "$TMP/SHA256SUMS" || die "download failed: $BASE/SHA256SUMS"
WANT=$(awk -v a="$ASSET" '$2 == a || $2 == "./" a { print $1 }' "$TMP/SHA256SUMS" | head -1)
[ -n "$WANT" ] || die "$ASSET not listed in SHA256SUMS"
GOT=$(sha256 "$TMP/$ASSET")
[ "$WANT" = "$GOT" ] || die "checksum mismatch for $ASSET (expected $WANT, got $GOT)"
chmod 0755 "$TMP/$ASSET"
"$TMP/$ASSET" version >/dev/null 2>&1 || die "downloaded binary does not run on this machine"
say "Checksum OK: $("$TMP/$ASSET" version)"

# ---- questions -----------------------------------------------------------------------------------------------
NEW_CONFIG=1
[ -f "$CONFIG" ] && NEW_CONFIG=0

if [ "$NEW_CONFIG" = 1 ]; then
	if [ -z "$(echo "$SERVERS" | tr -d ' ')" ]; then
		have_tty || die "no Xymon server given: pass --server HOST[:PORT] or set ZYMON_SERVER (no terminal to ask on)"
		while [ -z "$(echo "$SERVERS" | tr -d ' ')" ]; do
			ask "Xymon server (host or host:port, several separated by spaces)" ""
			SERVERS=$REPLY
		done
	fi
	if [ -z "$HOSTNAME_ARG" ]; then
		DETECTED=$(uname -n)
		ask "Host name as configured on the Xymon server" "$DETECTED"
		HOSTNAME_ARG=$REPLY
	fi
fi

# ---- service account -----------------------------------------------------------------------------------------
if [ "$OS" = Darwin ]; then
	# An unprivileged account: whoever invoked sudo, else the current user; root itself is never used.
	SVC_USER=${SUDO_USER:-$ME}
	[ "$SVC_USER" != root ] || SVC_USER=nobody
else
	SVC_USER=zymon
	if [ "$SERVICE" = 1 ] || [ "$(id -u)" = 0 ]; then
		if ! id "$SVC_USER" >/dev/null 2>&1; then
			step "Creating system user $SVC_USER"
			NOLOGIN=/usr/sbin/nologin
			[ -x "$NOLOGIN" ] || NOLOGIN=/sbin/nologin
			priv useradd --system --home-dir "$STATE_DIR" --no-create-home --shell "$NOLOGIN" "$SVC_USER"
		fi
		if [ -S /var/run/docker.sock ] && getent group docker >/dev/null 2>&1 && ! id -nG "$SVC_USER" | grep -qw docker; then
			if confirm "Docker found. Let $SVC_USER read the Docker socket (adds it to the docker group) to monitor containers?" n; then
				priv usermod -aG docker "$SVC_USER"
			fi
		fi
	else
		SVC_USER=$ME
	fi
fi
SVC_GROUP=$(id -gn "$SVC_USER" 2>/dev/null || echo "$SVC_USER")

# ---- install -------------------------------------------------------------------------------------------------
step "Installing to $BIN"
priv mkdir -p "$BINDIR"
priv install -m 0755 "$TMP/$ASSET" "$BIN"

priv mkdir -p "$CONFDIR" "$STATE_DIR"
priv chown "$SVC_USER:$SVC_GROUP" "$CONFDIR" "$STATE_DIR"

if [ "$NEW_CONFIG" = 1 ]; then
	step "Detecting this host and writing $CONFIG"
	set -- init --config "$CONFIG" --hostname "$HOSTNAME_ARG"
	for s in $SERVERS; do set -- "$@" --server "$s"; done
	as_user "$SVC_USER" "$BIN" "$@"
else
	step "Keeping existing config $CONFIG"
fi

step "Test run (nothing is sent)"
FIRST=$(as_user "$SVC_USER" "$BIN" once --config "$CONFIG" --dry-run 2>/dev/null | head -1) || true
[ -n "$FIRST" ] || die "zymon-client could not build a message; run: $BIN once --config $CONFIG --dry-run -v"
say "$FIRST"

# ---- service -------------------------------------------------------------------------------------------------
if [ "$SERVICE" = 1 ]; then
	if [ "$OS" = Darwin ]; then
		step "Installing launchd service"
		priv mkdir -p "$(dirname "$LOGFILE")"
		priv chown "$SVC_USER" "$(dirname "$LOGFILE")" 2>/dev/null || true
		cat >"$TMP/plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>org.zymon.client</string>
	<key>ProgramArguments</key>
	<array>
		<string>$BIN</string>
		<string>run</string>
		<string>--config</string>
		<string>$CONFIG</string>
	</array>
	<key>UserName</key>
	<string>$SVC_USER</string>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>StandardErrorPath</key>
	<string>$LOGFILE</string>
</dict>
</plist>
EOF
		priv launchctl bootout system "$PLIST" 2>/dev/null || true
		priv install -m 0644 -o root -g wheel "$TMP/plist" "$PLIST"
		priv launchctl bootstrap system "$PLIST"
		say "Service org.zymon.client started as $SVC_USER (log: $LOGFILE)"
	elif command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
		step "Installing systemd service"
		cat >"$TMP/unit" <<EOF
[Unit]
Description=zymon client (Xymon-compatible monitoring client)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$SVC_USER
ExecStart=$BIN run --config $CONFIG
ExecReload=/bin/kill -HUP \$MAINPID
Restart=on-failure
RestartSec=30
StateDirectory=zymon
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true

[Install]
WantedBy=multi-user.target
EOF
		priv install -m 0644 "$TMP/unit" "$UNIT"
		priv systemctl daemon-reload
		priv systemctl enable zymon-client >/dev/null 2>&1
		priv systemctl restart zymon-client
		say "Service zymon-client enabled and started (systemctl status zymon-client)"
	else
		say "No systemd found: not installing a service. Start it yourself with:"
		say "  $BIN run --config $CONFIG"
	fi
fi

step "Done"
say "Binary: $BIN"
say "Config: $CONFIG (edit, then: $([ "$OS" = Darwin ] && echo "sudo launchctl kickstart -k system/org.zymon.client" || echo "sudo systemctl reload zymon-client"))"
say "Make sure the host '${HOSTNAME_ARG:-from config}' exists in the Xymon server's hosts.cfg."
