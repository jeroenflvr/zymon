# AGENTS.md — zymon client (phase 1: client only)

zymon is a rewrite of Xymon in Rust. This file tells you how to build **the client
crate and the shared core crate**. There is **no server work in this phase** — the
client talks to an unmodified legacy Xymon 4.3.x server (`xymond`, the C code in the
parent directory of `zymon/`). Read [INSTRUCTIONS.md](./INSTRUCTIONS.md) first; it is
the human's brief and overrides anything here if they disagree.

Build in the phase order in §9. After each phase run its verify command and make it
pass before moving on.

---

## 1. Scope and mental model

- `zymon/core` → crate `zymon-core`: everything a client *and* a future server need:
  wire protocol builder/parser, TCP sender, hostname/OS tag helpers, tiny logger.
  No OS probing, no `/proc` reading, no config-file handling here.
- `zymon/client` → crate `zymon-client`, binary `zymon-client`: `init`, `once`,
  `run`. Collects system data, formats it **byte-compatible with the legacy shell
  client**, sends it to `xymond` on TCP 1984.
- `zymon/server` → **leave the directory empty.** Do not create a crate, a
  placeholder `main.rs`, or a workspace member for it.

The legacy server is the oracle. Whenever you are unsure what a section must look
like, read the shell script that produces it
([../client/xymonclient-linux.sh](../client/xymonclient-linux.sh),
[../client/xymonclient-darwin.sh](../client/xymonclient-darwin.sh),
[../client/xymonclient.sh](../client/xymonclient.sh)) and the C that parses it
([../xymond/client/linux.c](../xymond/client/linux.c),
[../xymond/client/darwin.c](../xymond/client/darwin.c),
[../xymond/xymond_client.c](../xymond/xymond_client.c),
[../xymond/rrd/do_vmstat.c](../xymond/rrd/do_vmstat.c)). Never guess a format.

---

## 2. Hard rules (acceptance criteria)

1. **Fast, light, simple.** Target is a Raspberry Pi 1 (700 MHz ARMv6, 512 MB
   shared RAM). A full collect+send cycle must finish in **< 2 s wall-clock on a
   Pi Zero W** and use **< 10 MB RSS** steady state. Measure, don't assume.
2. **No async runtime.** No `tokio`, `async-std`, `smol`. Blocking `std::net` with
   timeouts and one thread is all a monitoring client needs.
3. **Dependency allowlist.** `libc`, `serde` (+derive), `toml`, `lexopt`. Anything
   else needs a one-line justification in a code comment at the `use` site and must
   be pure Rust (no C build deps — they break cross builds). Explicitly forbidden:
   `sysinfo`, `nix`, `clap`, `regex`, `anyhow`, `thiserror`, `tracing`, `reqwest`,
   `openssl`. Hand-roll error enums; they are small.
4. **Byte-compatible sections.** Every section listed in §5 that the legacy server
   parses must produce output the legacy parser accepts. Golden tests (§8) enforce
   this. A section the server merely displays (`[top]`, `[mount]`, `[route]`, …) may
   differ cosmetically but must keep the same first-line layout.
5. **No secrets, no root assumptions.** The client runs as an unprivileged user.
   If a probe needs root it is skipped by `init` and never attempted at runtime.
6. **Static binaries for Linux.** All Linux targets build with musl and link
   statically (`task build` must produce binaries that `file` reports as
   "statically linked"). No glibc version coupling across Pi OS releases.
7. **`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` clean.**
   `unsafe` only in `core::net` (none expected) and in the `libc` FFI shims under
   `client/src/sys/`, each with a `// SAFETY:` comment.
8. **No entropy.** No traits where a function will do, no `Box<dyn …>` collector
   registries, no plugin system, no "framework". One function per section.

---

## 3. Toolchain and workspace

- Rust **stable** (pin with `rust-toolchain.toml`: `channel = "stable"`,
  `components = ["rustfmt", "clippy"]`, and the four targets from §7 under
  `targets`). Edition 2024.
- Cross-compilation: **`cargo-zigbuild`** (uses `zig` as the C linker — no Docker,
  works from macOS arm64 and Linux hosts). `cross` is the documented fallback only.
- Task runner: **go-task** (`Taskfile.yml`, §7). Install: `brew install go-task`
  (macOS) or `sh -c "$(curl -sL https://taskfile.dev/install.sh)" -- -d -b ~/.local/bin`.

`zymon/Cargo.toml` (workspace root):

```toml
[workspace]
resolver = "3"
members = ["core", "client"]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "GPL-2.0-only"          # xymon is GPLv2; stay compatible

[workspace.dependencies]              # versions indicative; add with `cargo add` to get current ones
libc   = "0.2"
serde  = { version = "1", features = ["derive"] }
toml   = "0.9"
lexopt = "0.3"

[profile.release]
opt-level = "s"                   # size over speed: Pi 1 is I-cache starved; measure "3" vs "s" once in Phase 6
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
debug = false
```

---

## 4. Project structure (create exactly this)

```
zymon/
├── AGENTS.md
├── INSTRUCTIONS.md
├── Taskfile.yml                 # §7
├── Cargo.toml                   # workspace
├── Cargo.lock
├── rust-toolchain.toml
├── .cargo/
│   └── config.toml              # per-target linker/rustflags (§7)
├── core/
│   ├── Cargo.toml               # [package] name = "zymon-core"
│   └── src/
│       ├── lib.rs
│       ├── proto.rs             # Message builders: client / status / combo / data; section writer
│       ├── net.rs               # send_message(): TCP, write, shutdown(Write), read reply to EOF
│       ├── host.rs              # hostname -> MACHINE (dots→commas), os tag, class
│       ├── clientcfg.rs         # parse server reply (logfetch.cfg lines: log:, file:, dir:, clientversion:)
│       └── log.rs               # 40-line stderr/file logger with levels, no deps
├── client/
│   ├── Cargo.toml               # [package] name = "zymon-client", [[bin]] name = "zymon-client"
│   ├── src/
│   │   ├── main.rs              # arg parsing (lexopt) → init | once | run | version
│   │   ├── config.rs            # Config struct, TOML load/save, defaults, paths
│   │   ├── init.rs              # system detection → Config
│   │   ├── schedule.rs          # run loop: interval, jitter, drift-free sleep, SIGTERM/SIGHUP
│   │   ├── collect.rs           # build_client_message(&Config, &mut State) -> Vec<u8>; section order
│   │   ├── state.rs             # cross-cycle state (prev /proc/stat, /proc/vmstat, net counters)
│   │   ├── sections/            # one file per section; fn <name>(ctx, w: &mut SectionWriter)
│   │   │   ├── mod.rs
│   │   │   ├── date.rs  uname.rs  osversion.rs  uptime.rs  who.rs
│   │   │   ├── df.rs    inode.rs  mount.rs      free.rs    meminfo.rs
│   │   │   ├── ifconfig.rs route.rs netstat.rs  ports.rs   ifstat.rs
│   │   │   ├── mdstat.rs ps.rs    nproc.rs      top.rs     vmstat.rs
│   │   │   ├── clock.rs clientversion.rs
│   │   │   ├── cpuinfo.rs       # zymon extra: /proc/cpuinfo Model/model name/Hardware/Revision… (for "info")
│   │   │   ├── crontab.rs       # zymon extra: system + user crontabs
│   │   │   └── docker.rs        # zymon extra: container inventory + client-side status
│   │   └── sys/                 # thin OS shims; nothing above this layer touches libc
│   │       ├── mod.rs
│   │       ├── linux.rs         # /proc + /sys readers, statvfs, getmntent-free /proc/self/mounts parser
│   │       ├── macos.rs         # sysctl(3), host_statistics64, getfsstat, getifaddrs
│   │       ├── exec.rs          # run(cmd, args, timeout) -> Output; PATH lookup; LANG=C
│   │       └── common.rs        # gethostname, getloadavg, uptime, time formatting
│   ├── packaging/
│   │   ├── zymon-client.service # systemd unit (Type=simple, User=zymon, ExecStart=... run)
│   │   ├── zymon-client.toml.example
│   │   └── org.zymon.client.plist   # launchd (macOS)
│   └── tests/
│       ├── golden/              # captured legacy outputs per OS/arch: linux-armv6l/, linux-aarch64/, darwin-arm64/
│       ├── golden_sections.rs   # §8
│       ├── legacy_parse.rs      # §8 — Rust port of the server's acceptance checks
│       └── fake_xymond.rs       # loopback TCP server used by core::net + end-to-end tests
└── server/                      # EMPTY in this phase
```

---

## 5. Legacy wire protocol (verified against xymon 4.3.30 source)

### 5.1 Transport (`core::net`)

- Plain TCP to `server:1984` (per-server `host:port` override allowed). No TLS.
- Write the whole message, then `shutdown(Shutdown::Write)`. `xymond` reads to EOF.
- A `client` message gets a **reply**: read to EOF with a read timeout (default 15 s,
  configurable). The reply is the host's `logfetch.cfg`-style config (may be empty).
- Connect timeout default 10 s; retry each server up to 3 times with 1 s backoff.
- Multiple servers (`servers = ["a", "b:1984"]`): send to **all** of them (legacy
  `XYMSERVERS` semantics), keep the first non-empty reply.
- `--dry-run` prints the message to stdout and sends nothing.
- Message bodies are bytes, not `String` — `ps`/`who` output is not guaranteed UTF-8.

### 5.2 `client` message

```
client[/COLLECTORID] MACHINE.OSTAG CLASS\n
[section]\n
…section body…\n
[section]\n
…
```

- `MACHINE` = hostname with every `.` replaced by `,` (`xymond` does
  `strrchr(hostname,'.')` to split off the OS tag, then `uncommafy()`).
- `OSTAG` = lowercase `uname -s` with `/`→`_` (`linux`, `darwin`), overridable in
  config (`os = "rhel"`), because it selects the server-side parser.
- `CLASS` defaults to `OSTAG`; overridable (`class = …`). Only the default client
  (no `/COLLECTORID`) may send a class.
- Sections start at column 0 with `[name]\n`. Names the server parses are listed in
  §5.4. Unknown names are stored and shown on the "client data" page — that is how
  the zymon extras ride along for free.
- Section order is fixed (§5.4); `[clientversion]` and `[clock]` are always **last**,
  in that order, because `[clock]` is used for clock-skew and must be sampled right
  before sending.

### 5.3 `status` / `combo` (used for client-side-evaluated columns, e.g. docker)

```
status[+LIFETIME] MACHINE.COLUMN COLOR <first line of text>\n
<body>\n
```

Several statuses go in one connection as `combo\n` + statuses separated by a blank
line (`\n\nstatus…`). Colors: `green yellow red clear`. `LIFETIME` e.g. `+10m`
(default 30 m server-side) — set it to `2 × interval` so a dead client goes purple.
`xymond` does **not** reply to `combo`/`status`; close after the write.

### 5.4 Sections — Linux (`[../client/xymonclient-linux.sh](../client/xymonclient-linux.sh)` order)

| Section | Server parser needs | Produce by |
|---|---|---|
| `[date]` | free text, shown | `date` format `%a %b %e %H:%M:%S %Z %Y` via libc `strftime` |
| `[uname]` | free text | `uname -rsmn` equivalent from `uname(2)` |
| `[osversion]` | free text, shown in info | first line of `/etc/os-release` `PRETTY_NAME` (fallback chain as the script) |
| `[uptime]` | line must contain `up N days, HH:MM,` **and** `load average: a, b, c` | `/proc/uptime` + `/proc/loadavg` + user count from `utmp`; copy `uptime(1)` layout exactly |
| `[who]` | free text (used for "users" count) | exec `who` (utmp parsing is not worth it) |
| `[df]` | `df -P` layout; header must contain `Available`, `Capacity`, `Mounted` | `/proc/self/mounts` + `statvfs`, excluding `nodev` fstypes and iso9660 exactly like the script; 1024-blocks |
| `[inode]` | `df -Pi` layout; header has `IFree`, `IUse%`, `Mounted` | same source, inode fields |
| `[mount]` | shown | `/proc/self/mounts` in `mount(8)` layout |
| `[free]` | **new-style procps** `free` (`total used free shared buff/cache available`) | `/proc/meminfo`; server divides by 1024 → emit **kB** like `free` without flags |
| `[ifconfig]` | shown | exec `ifconfig` if present (legacy layout); otherwise omit the section — `[ifstat]` carries the counters the server actually uses |
| `[route]` | shown | `/proc/net/route` in `netstat -rn` layout |
| `[netstat]` | `netstat -s` layout (TCP/UDP counters → RRD) | exec `netstat -s` if present; else synthesise the `Tcp:`/`Udp:` blocks from `/proc/net/snmp` in the same wording |
| `[ports]` | columns 3=local, 4=remote, 5=state (`netstat -ant` layout) | `/proc/net/tcp{,6}` + `/proc/net/udp{,6}`, format as `netstat -antuW` |
| `[ifstat]` | `ifconfig`-style `RX bytes:`/`TX bytes:` **or** `RX packets N bytes M` | `/proc/net/dev`, render the modern ifconfig layout |
| `[mdstat]` | raw `/proc/mdstat` (only if file exists) | copy file |
| `[ps]` | header must contain `CMD`; columns `pid,ppid,user,start,state,pri,pcpu,time,pmem,rsz,vsz,cmd` | exec `ps -Aww f -o …` (exactly the script's arg list); native `/proc` port is a Phase 6 option behind the golden test |
| `[nproc]` | integer | `sysconf(_SC_NPROCESSORS_CONF)` |
| `[top]` | display only (appended to cpu status) | **do not exec `top`** on Pi; emit the 5-line header (`top -`, `Tasks:`, `%Cpu(s):`, `MiB Mem`, `MiB Swap`) from `/proc` plus top-10 by CPU from the `[ps]` capture |
| `[vmstat]` | **last line** parsed, 17 numeric columns in `vmstat` linux layout (`r b swpd free buff cache si so bi bo in cs us sy id wa st`) | no 5-minute background `vmstat` — keep previous cycle's `/proc/stat`, `/proc/vmstat`, `/proc/meminfo` in `State`, emit header + one delta line. First cycle: omit section. |
| `[clientversion]` | string | `zymon-client/<cargo version>` |
| `[clock]` | exactly three lines: `epoch: %ld.%06ld`, `local: %Y-%m-%d %H:%M:%S %Z`, `UTC: %Y-%m-%d %H:%M:%S %Z` (see `logfetch --clock`) | `clock_gettime(REALTIME)` + `strftime` |

Linux extras (zymon-only, stored by the server, used later by the zymon server):

| Section | Content |
|---|---|
| `[cpuinfo]` | Parsed from **`/proc/cpuinfo`** (plus `/sys` and `/proc/device-tree` fallbacks), emitted as `key: value` lines in this fixed order: `model: ` (Pi: the `Model` line, e.g. `Raspberry Pi Zero W Rev 1.1`; fallback `/proc/device-tree/model`), `cpu_model: ` (`model name` on ARMv6/x86, else the first `CPU implementer`/`CPU part` pair decoded to a Cortex name, e.g. `Cortex-A53`), `cpu_arch: ` (`CPU architecture` line), `features: ` (`Features`/`flags` line), `cores: N` (count of `processor` entries), `bogomips: ` (first `BogoMIPS`), `hardware: ` (`Hardware` line, e.g. `BCM2835`; fallback `/proc/device-tree/compatible`), `revision: ` (`Revision` line, e.g. `9000c1`), `arch: ` (`uname -m`), `cpu_mhz_max: ` (`/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq` ÷ 1000 if readable), `throttled: 0x0` (`/sys/devices/platform/soc/soc:firmware/get_throttled` if readable). **Never emit the `Serial` line.** Omit a key when the source is missing; `/proc/cpuinfo` is read every cycle, not cached — it is cheap and lets a swapped SD card report correctly. |
| `[crontab]` | `/etc/crontab`, `/etc/cron.d/*`, and `crontab -l` of the running user, each prefixed `# file: <path>`; comments and blank lines stripped; deterministic order |
| `[docker]` | one line per container: `<name> <image> <state> <status>` via `GET /containers/json?all=1` on the unix socket (hand-rolled HTTP/1.1, no crate). Omit if socket absent/unreadable. |

### 5.5 Sections — macOS ([../client/xymonclient-darwin.sh](../client/xymonclient-darwin.sh))

`date, uname (-a), uptime, who, df, inode, mount, meminfo, ifconfig, route, netstat,
ifstat, ports, ps, nproc, top, [cpuinfo], [crontab], [docker], clientversion, clock`.

Differences that matter: `[df]` header words are `Avail`, `Capacity`, `Mounted`;
`[inode]` header `ifree`, `%iused`, `Mounted` (the script rewrites `df -i` into a
5-column layout — reproduce that layout, not raw `df -i`); `[meminfo]` is raw
`vm_stat` output (server reads `Pages free:`, `Pages active:`, `Pages inactive:`,
`Pages wired down:` and the page size from the first line) — use
`host_statistics64` and print those lines verbatim; `[ps]` header word is `COMMAND`;
`[ifstat]` is `netstat -ibn` minus `lo`/`<Link` lines; no `[free]`, no `[vmstat]`,
no `[mdstat]`. `[top]` on macOS: emit the `Processes:`/`Load Avg:`/`CPU usage:`/
`PhysMem:` header lines from sysctl/mach, not `top -l 2`. `[cpuinfo]` on macOS uses
the same keys as Linux, sourced from `sysctlbyname`: `model:` ← `hw.model`
(e.g. `Mac14,2`), `cpu_model:` ← `machdep.cpu.brand_string`, `cores:` ← `hw.ncpu`,
`cpu_mhz_max:` ← `hw.cpufrequency_max` ÷ 1e6 when present, `arch:` ← `hw.machine`;
no `hardware`/`revision`/`throttled`/`features` lines.

---

## 6. Client behaviour

### 6.1 Subcommands (`lexopt`, no subcommand framework)

```
zymon-client init  [--config PATH] [--server HOST[:PORT]]... [--hostname NAME] [--os TAG] [--class CLS] [--force]
zymon-client once  [--config PATH] [--dry-run] [--sections a,b,c]
zymon-client run   [--config PATH] [--dry-run]
zymon-client version
```

Default config path: `/etc/zymon/client.toml` (Linux), `/usr/local/etc/zymon/client.toml`
(macOS); `ZYMON_CLIENT_CONFIG` env overrides; `--config` overrides both. That env var
is the only environment variable the client reads besides `PATH`.

### 6.2 `init`

Runs once (refuses to overwrite without `--force`). Detects and writes:

```toml
# generated by zymon-client init on <date>; edit freely
[identity]
hostname = "pi4"            # MACHINEDOTS; dots are commafied on the wire
os = "linux"                # OSTAG sent to the server (selects the server-side parser)
class = "linux"

[servers]
list = ["192.168.1.10"]     # host or host:port
connect_timeout_s = 10
read_timeout_s = 15
retries = 3

[schedule]
interval_s = 300
jitter_s = 30               # random 0..jitter added once at startup

[sections]                  # every key present; false = detected but disabled, absent = not applicable on this host
date = true
uname = true
# … one key per section, in §5.4 order …
mdstat = true               # only emitted if /proc/mdstat exists at detect time
docker = false              # detected socket, but user is not in the docker group
crontab = true

[docker]
socket = "/var/run/docker.sock"
must_run = []               # container names; if non-empty, client sends `status MACHINE.docker` red when one is not running
status_column = "docker"

[info]                      # snapshot taken at init for the user's reference only; [cpuinfo] re-reads /proc/cpuinfo each cycle
model = "Raspberry Pi Zero W Rev 1.1"            # /proc/cpuinfo "Model"
cpu_model = "ARMv6-compatible processor rev 7 (v6l)"  # "model name" (or decoded CPU implementer/part)
cpu_arch = "7"                                   # "CPU architecture"
cores = 1
hardware = "BCM2835"                             # "Hardware"
revision = "9000c1"                              # "Revision"
arch = "armv6l"                                  # uname -m
```

Detection rules: a section is `true` only if its data source exists **and** is
readable by the current user **and** (for exec-backed sections) the binary is on a
fixed search path (`/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin`).
`init` prints a one-line summary per section (`df: ok`, `docker: skipped (socket not
readable)`). Nothing in `init` sends network traffic.

### 6.3 `run`

- Load config, apply jitter once, then loop: `collect → send → handle reply → sleep
  until next_tick`. `next_tick += interval` (drift-free, no accumulation).
- `SIGTERM`/`SIGINT` → exit after the current send. `SIGHUP` → reload config.
  Use a self-pipe or `libc::signal` with an `AtomicBool`; no signal crate.
- Each collector is wrapped: a failing section is **skipped with a log line**, never
  aborts the cycle. Exec-backed collectors have a hard timeout (default 20 s) and the
  child is killed on timeout.
- A collector writes into a shared `Vec<u8>` through `SectionWriter` which emits the
  `[name]\n` header lazily on first byte, so empty sections are not sent (matches
  legacy conditional sections).
- Reply handling (`core::clientcfg`): persist the raw reply to
  `<state_dir>/clientcfg.txt` (state dir: `/var/lib/zymon` or `$XDG_STATE_HOME/zymon`,
  fallback `/tmp/zymon-<uid>`). **Ignore** `clientversion:` — zymon never
  self-updates. `log:`/`file:`/`dir:` entries are **not acted on in this phase**
  (logfetch port is a later phase); log at debug level that they were received.
- If `docker.must_run` is non-empty, after the `client` message send a separate
  `combo` with one `status+<2×interval> MACHINE.<status_column> <color>` line listing
  each required container and its state.

### 6.4 `once`

Same as one `run` iteration, but exits 0 on success, 1 on send failure, 2 on config
error. `--dry-run` writes the exact bytes to stdout — this is what the golden tests
and the human will diff against the legacy script.

---

## 7. Build targets and Taskfile

| Hardware | SoC / core | 32-bit OS → target | 64-bit OS → target |
|---|---|---|---|
| Raspberry Pi 1 (A/B/A+/B+), Zero, Zero W | BCM2835, ARM1176 (ARMv6Z, VFPv2) | `arm-unknown-linux-musleabihf` | — |
| Zero 2 W, 3B/3B+ | BCM2710/2837, Cortex-A53 | `armv7-unknown-linux-musleabihf` | `aarch64-unknown-linux-musl` |
| 4B / 400 | BCM2711, Cortex-A72 | `armv7-unknown-linux-musleabihf` | `aarch64-unknown-linux-musl` |
| 5 | BCM2712, Cortex-A76 | `armv7-unknown-linux-musleabihf` | `aarch64-unknown-linux-musl` |
| Mac (Apple Silicon) | arm64 | — | `aarch64-apple-darwin` |

Notes you must respect:
- An `armv7` binary **crashes with SIGILL on Pi 1 / Zero W**. The `armv6` build runs
  on every 32-bit Pi OS, so it is the safe default for an installer that can't tell.
- `uname -m` → `armv6l` ⇒ armv6 build; `armv7l` ⇒ armv7; `aarch64` ⇒ aarch64 (a
  static aarch64 binary runs on a 64-bit kernel even with 32-bit userland).
- Darwin can only be built on a macOS host; the Taskfile guards it with `platforms:`.
- `rustflags` for armv6: `-C target-cpu=arm1176jzf-s`; armv7: `-C target-cpu=cortex-a7`
  (lowest common Cortex-A for Pi 2+); aarch64: default. Put them in `.cargo/config.toml`
  under `[target.<triple>]` and use `+crt-static` for all musl targets (default already,
  state it explicitly).

`Taskfile.yml` (create exactly this, then extend only by adding tasks):

```yaml
version: '3'

vars:
  BIN: zymon-client
  DIST: dist
  LINUX_TARGETS: arm-unknown-linux-musleabihf armv7-unknown-linux-musleabihf aarch64-unknown-linux-musl
  DARWIN_TARGET: aarch64-apple-darwin
  # dist/<label> names match `uname -m` on the Pi: armv6l -> linux-armv6, armv7l -> linux-armv7, aarch64 -> linux-aarch64

tasks:
  default:
    cmds: [task --list]

  setup:
    desc: Install Rust targets and cargo-zigbuild (requires rustup, zig on PATH)
    cmds:
      - rustup target add {{.LINUX_TARGETS}} {{.DARWIN_TARGET}}
      - cargo install cargo-zigbuild --locked
      - command -v zig >/dev/null || { echo "zig not found: brew install zig / apt install zig"; exit 1; }

  fmt:
    desc: rustfmt check
    cmds: [cargo fmt --all -- --check]

  lint:
    desc: clippy, deny warnings
    cmds: [cargo clippy --workspace --all-targets -- -D warnings]

  test:
    desc: unit + golden + loopback tests on the host
    cmds: [cargo test --workspace]

  check:
    desc: fmt + lint + test
    cmds:
      - task: fmt
      - task: lint
      - task: test

  build:linux:
    desc: Cross-build the client for all Raspberry Pi targets (static musl)
    cmds:
      - for: { var: LINUX_TARGETS }
        cmd: cargo zigbuild --release -p zymon-client --target {{.ITEM}}

  build:darwin:
    desc: Native build for Apple Silicon (macOS host only)
    platforms: [darwin]
    cmds:
      - cargo build --release -p zymon-client --target {{.DARWIN_TARGET}}

  build:
    desc: Build everything this host can build
    cmds:
      - task: build:linux
      - task: build:darwin

  dist:
    desc: Collect binaries into dist/<label>/zymon-client and write SHA256SUMS
    deps: [build]
    cmds:
      - rm -rf {{.DIST}} && mkdir -p {{.DIST}}
      - for: { var: LINUX_TARGETS }
        cmd: |
          case "{{.ITEM}}" in
            arm-unknown-linux-musleabihf)   label=linux-armv6 ;;
            armv7-unknown-linux-musleabihf) label=linux-armv7 ;;
            aarch64-unknown-linux-musl)     label=linux-aarch64 ;;
            *) echo "no label for {{.ITEM}}"; exit 1 ;;
          esac
          mkdir -p {{.DIST}}/$label
          cp target/{{.ITEM}}/release/{{.BIN}} {{.DIST}}/$label/
          file {{.DIST}}/$label/{{.BIN}} | grep -q 'statically linked' || { echo "{{.ITEM}} is not static"; exit 1; }
      - |
        if [ "$(uname -s)" = Darwin ] && [ -f target/{{.DARWIN_TARGET}}/release/{{.BIN}} ]; then
          mkdir -p {{.DIST}}/darwin-arm64 && cp target/{{.DARWIN_TARGET}}/release/{{.BIN}} {{.DIST}}/darwin-arm64/
        fi
      - cd {{.DIST}} && find . -type f -name {{.BIN}} -exec shasum -a 256 {} + > SHA256SUMS
      - ls -l {{.DIST}}/*/{{.BIN}}

  size:
    desc: Print stripped binary sizes per target
    cmds:
      - for: { var: LINUX_TARGETS }
        cmd: ls -l target/{{.ITEM}}/release/{{.BIN}} 2>/dev/null || true
      - ls -l target/{{.DARWIN_TARGET}}/release/{{.BIN}} 2>/dev/null || true

  clean:
    cmds:
      - cargo clean
      - rm -rf {{.DIST}}
```

If `cargo zigbuild` fails for a target (it should not for these four), the fallback is
`cross build --release -p zymon-client --target <triple>` — add a `build:linux:cross`
task rather than editing `build:linux`.

---

## 8. Testing strategy (cheap, deterministic, no root, no network)

1. **Unit tests** per parser: `/proc/meminfo` → `[free]`, `/proc/net/tcp` → `[ports]`
   line, `/proc/self/mounts` filtering, vmstat delta math. Inputs are literal strings
   in the test; no fixtures on disk for these.
2. **Golden tests** (`client/tests/golden_sections.rs`): for each OS/arch directory
   in `tests/golden/`, feed the captured raw inputs (`proc/meminfo`, `proc/stat`, …)
   through the collector and compare with the captured legacy script section output.
   Capture the fixtures **once per hardware class** with the legacy script
   (`sh ../client/xymonclient-linux.sh`) on a real Pi Zero W, a Pi 4/5 (64-bit), and a
   Mac; commit them. Comparison is line-structured, with a per-section allowlist of
   volatile columns (timestamps, counters, PIDs).
3. **Legacy acceptance tests** (`client/tests/legacy_parse.rs`): a Rust port of the
   *checks* the server performs — header words present (`CMD`, `Available`, `load
   average:`), column counts, numeric parse of the vmstat last line, `Pages free:`
   present on darwin — applied to `once --dry-run` output on the host running the
   tests. This is the test that catches regressions without a server.
4. **Loopback end-to-end** (`client/tests/fake_xymond.rs`): spin up a TCP listener on
   `127.0.0.1:0`, point the client at it, assert the first line is
   `client <machine>.<os> <class>`, sections are in §5.4 order, `[clock]` is last, and
   that the reply body is persisted to the state dir.
5. **Manual oracle** (the human runs it; make it trivial): `zymon-client once
   --dry-run > /tmp/z.txt; sh ../client/xymonclient-linux.sh > /tmp/x.txt; diff`.
   Provide `client/tests/diff-legacy.sh` that does exactly this with the right env
   (`LANG=C`, `TOP=`, `XYMONTMP=/tmp`, `MACHINEDOTS=$(uname -n)`).

---

## 9. Build order (do in sequence; verify each)

**Phase 0 — scaffold.** Workspace, both crates, `rust-toolchain.toml`,
`.cargo/config.toml`, `Taskfile.yml`, `zymon-client version`.
*Verify:* `task setup && task check && task build && task dist` succeed on the dev
Mac; `dist/` holds four binaries; `file` shows the three Linux ones as statically
linked ARM/AArch64 ELF.

**Phase 1 — core.** `proto` (client/status/combo builders, `SectionWriter`),
`host` (commafy, os tag), `net::send_message`, `clientcfg` parser, `log`.
*Verify:* `cargo test -p zymon-core`; loopback test proves write→shutdown→read-reply.

**Phase 2 — config + init.** `Config` TOML round-trip, defaults, path resolution,
`init` detection on Linux and macOS with the per-section summary.
*Verify:* `zymon-client init --config /tmp/c.toml --server 127.0.0.1` writes a file
with every §5.4 key present; running it twice without `--force` fails with exit 2.

**Phase 3 — Linux sections.** Implement every Linux row of §5.4 (exec-backed where
the table says so). Order and header spelling exactly as the table.
*Verify:* `legacy_parse.rs` green on a Linux box; `diff-legacy.sh` shows only
timestamp/counter differences on a Pi; `once --dry-run | wc -c` is within ±20 % of
the legacy script's output on the same host.

**Phase 4 — macOS sections.** Same for §5.5 on Apple Silicon.
*Verify:* same as Phase 3 on the Mac; `[meminfo]` lines match `vm_stat` names
exactly.

**Phase 5 — run loop + send + extras.** `schedule.rs`, signals, state dir, reply
persistence, `[cpuinfo]`, `[crontab]`, `[docker]` + optional docker `status` combo.
*Verify:* against a real legacy `xymond` the host turns green for `cpu disk inode
memory procs ports`, and the "client data" page shows every section including the
zymon extras; stopping the client turns the host purple after `2 × interval`.

**Phase 6 — Pi performance pass.** Measure on Pi Zero W: cycle wall time, RSS,
binary size. Decide `opt-level` 3 vs s by measurement, and whether to port `[ps]` to
native `/proc` (do it only if `ps` exec is > 25 % of cycle time).
*Verify:* numbers recorded in `client/README.md`; both budgets in §2 rule 1 met.

**Phase 7 — packaging + gates.** systemd unit, launchd plist, `README.md` (install
per Pi model via `uname -m`, config, `init`, running as service), `diff-legacy.sh`.
*Verify ALL:* `task check` clean, `task dist` passes the static check, all §8 tests
green, Phase 5 manual verification repeated from a `dist/` binary on at least one Pi
and one Mac.

---

## 10. Definition of done

`task dist` on a Mac produces static `zymon-client` binaries for armv6, armv7,
aarch64 Linux and a native arm64 macOS binary. On a fresh Raspberry Pi (any model
in §7) and on an Apple Silicon Mac: `zymon-client init --server <xymond>` detects the
host, `zymon-client run` (as a service) makes the host green on an unmodified Xymon
4.3.x server with the same columns the shell client produced, plus `[cpuinfo]`,
`[crontab]`, `[docker]` visible in the client data; a Pi Zero W cycle takes < 2 s and
< 10 MB RSS; clippy/fmt/tests are clean; the `server/` directory is still empty.

---

## 11. Decisions already made (change only if INSTRUCTIONS.md or the human says so)

- "info stores cpu type/model": implemented as the `[cpuinfo]` section + `[info]`
  table in config. The legacy server shows it on the client-data page; the zymon
  server will later render it on the `info` column.
- "crontabs": informational `[crontab]` section only; no client-side alerting.
- "dockers if supposed to stay running": inventory in `[docker]`; alerting only when
  `docker.must_run` is configured, delivered as a legacy `status` in a `combo`.
- `logfetch` (log/file/dir monitoring driven by the server reply) and `--local`
  mode (client-side `analysis.cfg` evaluation) are **out of scope** for this phase;
  the reply is persisted so the next phase can start from it.
- No self-update, ever. `clientversion:` in the reply is ignored.
