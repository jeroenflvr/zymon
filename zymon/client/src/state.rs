//! Cross-cycle state: the previous counter snapshot, so `[vmstat]` and `[top]` can report deltas.

use std::time::Instant;

/// CPU jiffies in /proc/stat order: user nice system idle iowait irq softirq steal.
/// On macOS only user/nice/system/idle are filled.
pub type CpuTimes = [u64; 8];

#[derive(Clone, Debug)]
pub struct Snap {
    pub at: Instant,
    pub cpu: CpuTimes,
    pub ctxt: u64,
    pub intr: u64,
    pub procs_running: u64,
    pub procs_blocked: u64,
    pub pgpgin: u64,
    pub pgpgout: u64,
    pub pswpin: u64,
    pub pswpout: u64,
}

#[derive(Default)]
pub struct State {
    pub prev: Option<Snap>,
}
