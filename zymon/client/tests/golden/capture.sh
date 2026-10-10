#!/bin/sh
# Capture golden fixtures for one hardware class with the LEGACY shell client.
#   sh capture.sh golden/linux-armv6l      # on a Pi Zero W
#   sh capture.sh golden/linux-aarch64     # on a Pi 4/5 (64-bit OS)
#   sh capture.sh golden/darwin-arm64      # on an Apple Silicon Mac
# Writes raw /proc inputs (Linux) and the legacy per-section output (legacy/<section>.txt).
# Nothing host-identifying is kept: no [who], [ps], [ifconfig] or hostnames.
set -e
out=${1:?usage: capture.sh OUTDIR}
here=$(cd "$(dirname "$0")" && pwd)
legacy=${LEGACY_DIR:-$here/../../../../client}
mkdir -p "$out/legacy"

case $(uname -s) in
Linux)
	script=xymonclient-linux.sh
	keep="uptime df inode mount free route netstat ports ifstat mdstat"
	mkdir -p "$out/proc/net"
	for f in meminfo stat vmstat cpuinfo loadavg uptime filesystems; do cp /proc/$f "$out/proc/$f"; done
	cp /proc/self/mounts "$out/proc/mounts"
	for f in route tcp tcp6 udp udp6 dev snmp; do [ -r /proc/net/$f ] && cp /proc/net/$f "$out/proc/net/$f"; done
	[ -r /proc/mdstat ] && cp /proc/mdstat "$out/proc/mdstat" || true
	;;
Darwin)
	script=xymonclient-darwin.sh
	keep="uptime df inode mount meminfo"
	;;
*) echo "unsupported OS" >&2; exit 1 ;;
esac

tmp=$(mktemp -d)
# the legacy script starts a 5-minute background vmstat; a no-op shim keeps the capture quick and clean
mkdir -p "$tmp/bin" && printf '#!/bin/sh\nexit 0\n' > "$tmp/bin/vmstat" && chmod +x "$tmp/bin/vmstat"
PATH="$tmp/bin:$PATH" LANG=C TOP= XYMONTMP="$tmp" MACHINEDOTS=$(uname -n) sh "$legacy/$script" > "$tmp/all.txt"
awk -v out="$out/legacy" -v keep=" $keep " '
	/^\[.*\]$/ { name = substr($0, 2, length($0) - 2); file = (index(keep, " " name " ") ? out "/" name ".txt" : ""); if (file != "") printf "" > file; next }
	file != "" { print >> file }
' "$tmp/all.txt"
rm -rf "$tmp"
echo "captured into $out"
