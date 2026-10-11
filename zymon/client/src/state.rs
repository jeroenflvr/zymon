//! Cross-cycle state: the previous counter snapshot, so `[vmstat]` and `[top]` can report deltas, plus the
//! `[smart]` and `[gpu]` captures of the current cycle for the status columns that follows the client message.

use crate::sections::gpu::Card;
use crate::sections::smart::Disk;
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
    /// Set by the `[smart]` section, taken by the `smart` status column (smartctl runs once per cycle).
    pub smart: Option<Vec<Disk>>,
    /// Set by the `[gpu]` section, taken by the `gpu` status column (nvidia-smi runs once per cycle).
    pub gpu: Option<Result<Vec<Card>, String>>,
}
