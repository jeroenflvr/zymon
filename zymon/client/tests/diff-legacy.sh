#!/bin/sh
# Manual oracle: compare `zymon-client once --dry-run` with the legacy shell client on this host.
#   sh client/tests/diff-legacy.sh [path/to/zymon-client] [path/to/client.toml]
# Expect differences only in timestamps, counters, PIDs and the zymon extras ([cpuinfo] [crontab] [docker]).
set -e
here=$(cd "$(dirname "$0")" && pwd)
bin=${1:-$here/../../target/release/zymon-client}
legacy_dir=${LEGACY_DIR:-$here/../../../client}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

cfg=${2:-}
if [ -z "$cfg" ]; then
	cfg=$tmp/client.toml
	"$bin" init --config "$cfg" --server 127.0.0.1 >/dev/null
fi

case $(uname -s) in
Linux) script=xymonclient-linux.sh ;;
Darwin) script=xymonclient-darwin.sh ;;
*) echo "unsupported OS" >&2; exit 1 ;;
esac

"$bin" once --config "$cfg" --dry-run > /tmp/z.txt

# The legacy script starts a 5-minute background vmstat; a no-op shim keeps this run quick and clean.
mkdir -p "$tmp/bin" && printf '#!/bin/sh\nexit 0\n' > "$tmp/bin/vmstat" && chmod +x "$tmp/bin/vmstat"
PATH="$tmp/bin:$PATH" LANG=C TOP= XYMONTMP=/tmp MACHINEDOTS=$(uname -n) sh "$legacy_dir/$script" > /tmp/x.txt

echo "== bytes per section (legacy / zymon)"
for f in /tmp/x.txt /tmp/z.txt; do
	awk '/^\[.*\]$/ { name = $0; next } { n[name] += length($0) + 1 } END { for (k in n) printf "%s %d\n", k, n[k] }' "$f" | sort > "$tmp/$(basename "$f").sizes"
done
join -a1 -a2 -e 0 -o 0,1.2,2.2 "$tmp/x.txt.sizes" "$tmp/z.txt.sizes" | awk '{ printf "%-16s %8d %8d\n", $1, $2, $3 }'

lx=$(wc -c < /tmp/x.txt)
zx=$(wc -c < /tmp/z.txt)
echo "== total: legacy $lx bytes, zymon $zx bytes ($(( zx * 100 / lx ))% of legacy; target 80-120%)"
echo "== diff /tmp/x.txt (legacy) /tmp/z.txt (zymon)"
diff /tmp/x.txt /tmp/z.txt || true
