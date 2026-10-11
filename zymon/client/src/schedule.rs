//! One collect+send cycle (`once`) and the drift-free run loop.

use crate::collect::build_client_message;
use crate::config::{self, Config};
use crate::sections::{cputemp, docker, gpu, smart};
use crate::state::State;
use crate::sys::common;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use zymon_core::clientcfg::{self, Entry};
use zymon_core::{net, proto};

/// Persist the server's reply for the later logfetch phase. Written atomically.
fn save_reply(dir: &Path, reply: &[u8]) {
    let tmp = dir.join("clientcfg.txt.tmp");
    let dst = dir.join("clientcfg.txt");
    if let Err(e) = std::fs::write(&tmp, reply).and_then(|_| std::fs::rename(&tmp, &dst)) {
        zymon_core::warn!("cannot save {}: {e}", dst.display());
    }
}

/// Ignore `clientversion:` (no self-update, ever); `log:`/`file:`/`dir:` are for a later phase.
fn handle_reply(dir: &Path, reply: &[u8]) {
    if reply.is_empty() {
        return;
    }
    save_reply(dir, reply);
    let entries = clientcfg::parse(reply);
    let n = |f: fn(&Entry) -> bool| entries.iter().filter(|e| f(e)).count();
    zymon_core::debug!(
        "server config received: {} log, {} file, {} dir entries, {} other (not acted on)",
        n(|e| matches!(e, Entry::Log(_))),
        n(|e| matches!(e, Entry::File(_))),
        n(|e| matches!(e, Entry::Dir(_))),
        n(|e| matches!(e, Entry::Other(_)))
    );
}

pub struct CycleOpts<'a> {
    pub dry_run: bool,
    pub only: Option<&'a [String]>,
    pub state_dir: &'a Path,
}

/// Columns the client evaluates itself, sent as one `combo`: `docker` (when `docker.must_run` is set), `cputemp`
/// and `smart`/`gpu` (from the `[smart]`/`[gpu]` captures of this cycle).
fn client_side_statuses(cfg: &Config, state: &mut State) -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    if !cfg.docker.must_run.is_empty()
        && let Some(s) =
            docker::must_run_status(cfg, &docker::query(&cfg.docker.socket, cfg.exec_timeout()))
    {
        v.push(s);
    }
    if cfg.section_enabled("cputemp")
        && let Some(s) = cputemp::current_status(cfg)
    {
        v.push(s);
    }
    if let Some(disks) = state.smart.take()
        && let Some(s) = smart::status_message(cfg, &disks)
    {
        v.push(s);
    }
    if let Some(cards) = state.gpu.take()
        && let Some(s) = gpu::status_message(cfg, &cards)
    {
        v.push(s);
    }
    v
}

/// Collect, send, handle the reply, then the optional docker status. True when at least one server
/// accepted the client message (or in dry-run mode).
pub fn cycle(cfg: &Config, state: &mut State, o: &CycleOpts) -> bool {
    let msg = build_client_message(cfg, state, o.only);
    let extra = if o.only.is_none() {
        client_side_statuses(cfg, state)
    } else {
        state.smart = None;
        state.gpu = None;
        Vec::new()
    };

    if o.dry_run {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(&msg);
        let _ = out.flush();
        if !extra.is_empty() {
            eprintln!(
                "--- client-side status columns (separate combo message, not part of the client message) ---"
            );
            let _ = std::io::stderr().write_all(&proto::combo(&extra));
        }
        return true;
    }

    let servers = &cfg.servers.list;
    let opts = cfg.net_opts();
    let (reply, errors) = net::send_all(servers, &msg, true, &opts);
    for e in &errors {
        zymon_core::error!("{e}");
    }
    let ok = errors.len() < servers.len();
    if ok {
        handle_reply(o.state_dir, &reply);
    }
    if !extra.is_empty() {
        let (_, errs) = net::send_all(servers, &proto::combo(&extra), false, &opts);
        for e in &errs {
            zymon_core::error!("status columns: {e}");
        }
    }
    ok
}

/// Pseudo-random 0..=max seconds without an RNG crate: clock nanoseconds mixed with the pid.
fn jitter(max_s: u64) -> Duration {
    if max_s == 0 {
        return Duration::ZERO;
    }
    let (s, us) = common::now();
    let mut x = (s as u64) ^ ((us as u64) << 20) ^ ((std::process::id() as u64) << 40);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    Duration::from_millis(x % (max_s * 1000 + 1))
}

fn reload(path: &Path, cfg: &mut Config) {
    match Config::load(path) {
        Ok(c) => {
            *cfg = c;
            zymon_core::info!("config reloaded from {}", path.display());
        }
        Err(e) => zymon_core::error!("reload failed, keeping the old config: {e}"),
    }
}

pub fn run(mut cfg: Config, path: PathBuf, dry_run: bool) -> i32 {
    if let Err(e) = common::install_signals() {
        zymon_core::error!("cannot install signal handlers: {e}");
        return 1;
    }
    let state_dir = config::state_dir(&cfg);
    let mut state = State::default();
    let j = jitter(cfg.schedule.jitter_s);
    zymon_core::info!(
        "zymon-client started, first cycle in {:.1}s",
        j.as_secs_f64()
    );
    common::wait_for_event(j);
    let mut next = Instant::now();
    while !common::stop_requested() {
        if common::take_reload() {
            reload(&path, &mut cfg);
        }
        let t = Instant::now();
        let opts = CycleOpts {
            dry_run,
            only: None,
            state_dir: &state_dir,
        };
        let ok = cycle(&cfg, &mut state, &opts);
        zymon_core::debug!("cycle done in {:?} (sent: {ok})", t.elapsed());
        if common::stop_requested() {
            break;
        }
        // Drift-free: ticks stay on the original grid; a cycle that overran skips ahead instead of bursting.
        let interval = Duration::from_secs(cfg.schedule.interval_s);
        next += interval;
        let now = Instant::now();
        while next <= now {
            next += interval;
        }
        loop {
            let now = Instant::now();
            if now >= next || common::stop_requested() {
                break;
            }
            common::wait_for_event(next - now);
            if common::take_reload() {
                reload(&path, &mut cfg);
            }
        }
    }
    zymon_core::info!("zymon-client stopping");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_bounds() {
        assert_eq!(jitter(0), Duration::ZERO);
        for _ in 0..50 {
            assert!(jitter(3) <= Duration::from_secs(3));
        }
    }
}
