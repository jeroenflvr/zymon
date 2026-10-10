# zymon-client: measurements and notes

Budget (AGENTS.md section 2): a full collect+send cycle in under 2 s on a Pi Zero W, under 10 MB RSS.

## Measured so far

Real Pi hardware was not available while writing this; everything below is from a dev Mac and an arm64 Linux VM.
**To do on a Pi Zero W:** `time zymon-client once --dry-run >/dev/null`, `grep VmHWM /proc/$(pidof zymon-client)/status` while
`run` is going, and `-v` for per-section timings (`DEBUG section <name>: <time>` on stderr).

| What | Result |
|---|---|
| Linux aarch64 (Debian bookworm VM, procps + net-tools), `run` | cycle 6-10 ms, VmHWM 1.4 MB, SIGTERM to exit 5 ms |
| macOS (M1), `once --dry-run` | ~115 ms wall, of which `ps` 78 ms (the section is exec-backed there) |
| armv6 / armv7 binaries | run under qemu (`--platform linux/arm/v7`), date/clock output correct |
| Binary size (stripped, static) | armv6 799 kB (`opt-level = "s"`), 883 kB with `"3"`; aarch64 755 kB; armv7 789 kB; macOS arm64 638 kB |

## Decisions pending real-hardware data

* **opt-level `"s"` vs `"3"`**: kept `"s"` (11 % smaller). Rebuild with `CARGO_PROFILE_RELEASE_OPT_LEVEL=3 cargo zigbuild --release -p zymon-client --target arm-unknown-linux-musleabihf` and compare cycle times on the Zero W.
* **Native `/proc` port of `[ps]`**: not done. The rule is to do it only if the `ps` exec exceeds 25 % of the cycle on the Pi. On the VM it is
  ~2 ms of a ~7 ms cycle; Pi OS always ships procps, so the exec path is the default. Hosts without procps (busybox) simply
  report no `[ps]` section (`init` marks it not applicable).
* Exec-backed sections on Linux: `who`, `ps`, `netstat -s` (only when net-tools is installed), `ifconfig` (same), `crontab -l`.
  Everything else reads `/proc`/`/sys` directly. On macOS most sections exec the BSD tools, as the legacy script does.

## Behaviour notes

* `[vmstat]` needs a previous snapshot, so it is absent from `once` and from the first `run` cycle.
* `[df]`/`[inode]` on macOS skip non-local filesystems (a stuck NFS mount would block `statvfs`, as it does the legacy `df`) and keep
  the read-only system root `/`, which the legacy script's filter drops on modern macOS. Spaces in names become `_`.
* `[free]` reports used = total - free - buff/cache; the server adds buff/cache back for its classic "used" thresholds.
* On 32-bit ARM the `libc` crate still uses 32-bit `time_t`, matched by musl's time32 compatibility symbols (fine until 2038).
