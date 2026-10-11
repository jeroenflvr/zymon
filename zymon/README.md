# zymon

A Rust rewrite of Xymon. Phase 1 is the **client**: it detects what a host can report, collects it in the
legacy client format and sends it to an unmodified Xymon 4.3.x server (`xymond`, TCP 1984).

| Path | Crate | Contents |
|---|---|---|
| `core/` | `zymon-core` | wire protocol, TCP sender, host naming, reply parser, logger (shared with the future server) |
| `client/` | `zymon-client` | `init`, `once`, `run`; one function per `[section]` |
| `server/` | - | empty in this phase |

See [AGENTS.md](./AGENTS.md) / [INSTRUCTIONS.md](./INSTRUCTIONS.md) for the brief and [client/README.md](./client/README.md)
for measurements.

## Build

```sh
task setup     # rustup targets + cargo-zigbuild (needs zig on PATH)
task check     # fmt + clippy -D warnings + tests
task dist      # dist/linux-armv6, linux-armv7, linux-aarch64, linux-x86_64 (static musl) and darwin-arm64 + SHA256SUMS
```

`task` is [go-task](https://taskfile.dev): `brew install go-task` or
`sh -c "$(curl -sL https://taskfile.dev/install.sh)" -- -d -b ~/.local/bin`.

## Install

One-liner (Linux ARM and Apple Silicon; asks for the Xymon server, then installs, configures and starts the service):

```sh
curl -fsSL https://raw.githubusercontent.com/jeroenflvr/zymon/main/zymon/install.sh | sh
curl -fsSL https://raw.githubusercontent.com/jeroenflvr/zymon/main/zymon/install.sh | sh -s -- --server xymon.example.org --yes
```

[install.sh](./install.sh) detects OS/arch, downloads the matching `zymon-client-<label>` from the GitHub release, verifies it against
`SHA256SUMS`, installs to `/usr/local/bin`, runs `zymon-client init` as an unprivileged service user (`zymon` on Linux, the invoking user
on macOS), and installs a systemd unit or launchd daemon. When it finds a Docker socket or `smartctl` it offers to let the service user use
them (docker group membership, a `sudoers.d` rule for smartctl). Re-running upgrades the binary and keeps the config. `--help` lists the options
(`--hostname`, `--version`, `--prefix`, `--no-service`, `--uninstall [--purge]`). `task release` builds the flat release assets
(`dist/release/`) to attach to a GitHub release.

Manual install:

Pick the binary by `uname -m` on the target:

| `uname -m` | Hardware | `dist/` directory |
|---|---|---|
| `armv6l` | Pi 1, Zero, Zero W (also safe on any 32-bit Pi OS) | `linux-armv6` |
| `armv7l` | Pi 2, Zero 2 W, 3, 4, 5 with 32-bit Pi OS | `linux-armv7` |
| `aarch64` | Pi 3, 4, 5, Zero 2 W with 64-bit Pi OS | `linux-aarch64` |
| `x86_64` | Intel/AMD PCs and servers, VMs | `linux-x86_64` |
| `arm64` (macOS) | Apple Silicon | `darwin-arm64` |

An `armv7` binary dies with SIGILL on a Pi 1 / Zero W; when in doubt use `armv6`.

```sh
sudo install -m 0755 dist/linux-armv6/zymon-client /usr/local/bin/        # use your directory
sudo useradd --system --home /var/lib/zymon --shell /usr/sbin/nologin zymon
sudo mkdir -p /etc/zymon && sudo chown zymon /etc/zymon
sudo -u zymon zymon-client init --server xymon.example.org               # detects the host, writes /etc/zymon/client.toml
sudo -u zymon zymon-client once --dry-run | less                          # exactly what would be sent
sudo install -m 0644 client/packaging/zymon-client.service /etc/systemd/system/
sudo systemctl enable --now zymon-client
```

macOS: install to `/usr/local/bin`, `zymon-client init --server ... ` (writes `/usr/local/etc/zymon/client.toml`; create the
directory first), then copy `client/packaging/org.zymon.client.plist` to `/Library/LaunchDaemons/`, adjust `UserName`
(`_zymon` must exist; your own login also works) and `sudo launchctl load -w /Library/LaunchDaemons/org.zymon.client.plist`.

The hostname in the config must match the host in the server's `hosts.cfg`. Nothing else is needed on the server.

## Usage

```
zymon-client init  [--config PATH] [--server HOST[:PORT]]... [--hostname NAME] [--os TAG] [--class CLS] [--force]
zymon-client once  [--config PATH] [--dry-run] [--sections a,b,c] [-v]     exit 0 sent, 1 send failure, 2 config error
zymon-client run   [--config PATH] [--dry-run] [-v]                        SIGTERM/SIGINT stop, SIGHUP reloads the config
zymon-client restart                                                        restarts the installed service (systemctl / launchctl, via sudo if needed)
zymon-client version
```

Config path: `--config`, else `$ZYMON_CLIENT_CONFIG`, else `/etc/zymon/client.toml` (macOS `/usr/local/etc/zymon/client.toml`).
`init` refuses to overwrite an existing file without `--force` and never touches the network.
The client also reads `$XDG_STATE_HOME`, only to find a writable state directory when `/var/lib/zymon` is not.

The sample [client/packaging/zymon-client.toml.example](./client/packaging/zymon-client.toml.example) documents every key.

* `[sections]`: one key per section; `false` = detected but disabled, no key = not applicable here.
  A section needing root is skipped by `init` and never attempted.
* `docker.must_run = ["web"]`: after each client message a `status+<2*interval> HOST.docker` combo goes out, red when a
  listed container is missing or not running. The inventory itself is the `[docker]` client section.
* The server's reply (a `logfetch.cfg`) is stored in `<state dir>/clientcfg.txt`. `log:`/`file:`/`dir:` entries are not acted on yet,
  and `clientversion:` is ignored: zymon never updates itself.

* `cputemp` column (Linux): `[cputemp]` lists every thermal sensor (`name: degrees C`). Each cycle a client-side `status+<2*interval> HOST.cputemp`
  goes out in the layout of the original hand-made test, so existing graphs keep working: the date as the first line, then
  `CPU temperature 42.4 C`, `temp : 42.4` (the line the server's NCV handler graphs as data set `temp`) and
  `Yellow from 70 C, red from 80 C`. The value is the hottest CPU sensor; the colour turns yellow at `[cputemp] warn_c` (70), red at `panic_c` (80),
  and at least yellow while the Pi firmware reports throttling or under-voltage (an extra `&yellow firmware: ...` line, ignored by NCV).
  Hosts without a sensor (VMs, macOS) send neither.
* `smart` column: `[smart]` lists every disk smartmontools can see, one line per disk:
  `<name> <PASSED|FAILED|-> <temp C> <power-on hours> <model> <serial> [# problems; note]`. Each cycle a client-side
  `status+<2*interval> HOST.smart` goes out: red when `smartctl -H` reports a failed health check (NVMe critical warning included),
  yellow for reallocated/pending/uncorrectable sectors, media errors, SCSI grown defects, attributes that failed, or NVMe wear at 90 % or
  more; clear for disks in standby (they are never spun up: `-n standby`) or without SMART. The body ends with `<name>_temp : <C>` lines,
  so `NCV_smart="sda_temp:GAUGE,..."` graphs the temperatures. Disks come from `smartctl --scan-open`, or `[smart] devices` lists them
  with any `-d TYPE` smartctl needs. `smartctl` needs raw device access: run as root or add a sudoers rule
  (`zymon ALL=(root) NOPASSWD: /usr/sbin/smartctl`; `install.sh` offers to write it) — the client then calls `sudo -n smartctl`. The systemd unit must not set `NoNewPrivileges=true` (sudo is setuid and would fail);
  `install.sh` leaves it out when it wrote the sudoers rule.
  `init` leaves the section out when smartctl is missing and sets it to `false` when it cannot run it.
* `gpu` column (Linux, discrete NVIDIA cards): `[gpu]` has one line per card from a single `nvidia-smi --query-gpu` call:
  `gpu<N> <temp C> <util %> <mem used MiB> <mem total MiB> <power W> <bus id> <name> [# fault]` (`-` where the card reports `[N/A]`).
  Each cycle a client-side `status+<2*interval> HOST.gpu` goes out: yellow/red when a card reaches `[gpu] temp_warn_c`/`temp_panic_c`
  (80/90 C) or `mem_warn_pct`/`mem_panic_pct` (90/98 % of its memory in use; frameworks that reserve all GPU memory up front need these
  raised above 100), red when nvidia-smi fails (driver not loaded, card fallen off the bus) or reports a card as lost. The body ends with
  `gpu<N>_temp`, `gpu<N>_util` and `gpu<N>_mem` (percent used) lines, so `NCV_gpu="*:GAUGE"` (and `gpu=ncv` in `TEST2RRD`) graphs them.
  nvidia-smi needs no root. Integrated GPUs (Jetson/Tegra, no dedicated memory) are left out; `init` leaves the section out when
  nvidia-smi is missing or sees no discrete card, and sets it to `false` when nvidia-smi fails.

Besides the legacy sections the client sends `[cpuinfo]`, `[cputemp]`, `[gpu]`, `[crontab]`, `[docker]` and `[smart]`; the legacy server keeps them and shows
them on the "client data" page.

## Tests

* unit tests per parser (literal inputs), `cargo test --workspace`
* `client/tests/legacy_parse.rs`: the server's acceptance checks applied to `once --dry-run` on the test host
* `client/tests/fake_xymond.rs`: loopback server, real binary: header, section order, `[clock]` last, reply persisted, exit codes
* `client/tests/golden_sections.rs`: legacy-script output captured per hardware class in `client/tests/golden/`
  (`capture.sh` captures a new class on real hardware; `linux-aarch64` and `darwin-arm64` are committed)
* `sh client/tests/diff-legacy.sh`: the manual oracle: section sizes and a diff against the legacy shell client on this host
