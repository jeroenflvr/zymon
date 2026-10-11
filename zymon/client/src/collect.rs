//! Build the `client` message: fixed section order, a failing section is logged and skipped.

use crate::config::Config;
use crate::sections::{self, Ctx};
use crate::state::State;
use std::time::Instant;
use zymon_core::proto::{self, SectionWriter};

#[cfg(target_os = "linux")]
fn take_snap() -> Option<crate::state::Snap> {
    crate::sys::linux::snap().ok()
}

#[cfg(target_os = "macos")]
fn take_snap() -> Option<crate::state::Snap> {
    let t = crate::sys::macos::cpu_ticks().ok()?;
    Some(crate::state::Snap {
        at: Instant::now(),
        cpu: [t[0], t[1], t[2], t[3], 0, 0, 0, 0],
        ctxt: 0,
        intr: 0,
        procs_running: 0,
        procs_blocked: 0,
        pgpgin: 0,
        pgpgout: 0,
        pswpin: 0,
        pswpout: 0,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn take_snap() -> Option<crate::state::Snap> {
    None
}

/// `only` (from `once --sections`) overrides the config: exactly those sections, in wire order.
pub fn build_client_message(cfg: &Config, state: &mut State, only: Option<&[String]>) -> Vec<u8> {
    let wanted = |name: &str| match only {
        Some(list) => list.iter().any(|s| s == name),
        None => cfg.section_enabled(name),
    };
    let order = sections::order();
    let need_snap = order
        .iter()
        .any(|n| matches!(*n, "top" | "vmstat") && wanted(n));
    let cur = if need_snap { take_snap() } else { None };
    let mut ctx = Ctx::new(cfg, state.prev.take(), cur);

    let mut msg = proto::client_header(None, &cfg.machine(), &cfg.identity.os, cfg.class());
    for name in order {
        if !wanted(name) {
            continue;
        }
        let t = Instant::now();
        let mut w = SectionWriter::new(&mut msg, name);
        match sections::run(name, &mut ctx, &mut w) {
            Ok(()) => {
                w.finish();
            }
            Err(e) => {
                w.abort();
                zymon_core::warn!("section {name} skipped: {e}");
            }
        }
        zymon_core::debug!("section {name}: {:?}", t.elapsed());
    }
    state.prev = ctx.cur.take();
    state.smart = ctx.smart.take();
    state.gpu = ctx.gpu.take();
    msg
}
