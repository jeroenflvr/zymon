//! macOS: sysctl(3) and mach host statistics.

use std::ffi::CString;
use std::io;

fn sysctl_raw(name: &str, buf: &mut [u8]) -> Option<usize> {
    let c = CString::new(name).ok()?;
    let mut len = buf.len();
    // SAFETY: `buf` is valid for `len` bytes; the kernel updates `len` to the size written.
    let rc = unsafe {
        libc::sysctlbyname(
            c.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0).then_some(len)
}

pub fn sysctl_string(name: &str) -> Option<String> {
    let mut b = [0u8; 256];
    let n = sysctl_raw(name, &mut b)?;
    let s = &b[..n];
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    Some(String::from_utf8_lossy(&s[..end]).into_owned())
}

pub fn sysctl_u64(name: &str) -> Option<u64> {
    let mut b = [0u8; 8];
    match sysctl_raw(name, &mut b)? {
        4 => Some(u32::from_ne_bytes(b[..4].try_into().ok()?) as u64),
        8 => Some(u64::from_ne_bytes(b)),
        _ => None,
    }
}

/// Boot time as epoch seconds (`kern.boottime` is a `struct timeval`).
pub fn boottime() -> io::Result<i64> {
    let mut b = [0u8; 16];
    match sysctl_raw("kern.boottime", &mut b) {
        Some(n) if n >= 8 => Ok(i64::from_ne_bytes(b[..8].try_into().expect("8 bytes"))),
        _ => Err(io::Error::other("kern.boottime unavailable")),
    }
}

/// Counters `vm_stat` prints, in pages.
#[derive(Default, Debug, Clone)]
pub struct VmStats {
    pub page_size: u64,
    pub free: u64,
    pub active: u64,
    pub inactive: u64,
    pub speculative: u64,
    pub throttled: u64,
    pub wired: u64,
    pub purgeable: u64,
    pub faults: u64,
    pub cow: u64,
    pub zero_fill: u64,
    pub reactivated: u64,
    pub purged: u64,
    pub file_backed: u64,
    pub anonymous: u64,
    pub compressor_stored: u64,
    pub compressor_pages: u64,
    pub decompressions: u64,
    pub compressions: u64,
    pub pageins: u64,
    pub pageouts: u64,
    pub swapins: u64,
    pub swapouts: u64,
}

pub fn vm_stats() -> io::Result<VmStats> {
    // SAFETY: zeroed vm_statistics64 is a valid out-parameter for host_statistics64.
    let mut s: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: `s` is at least `count` integers wide; mach_host_self returns the send right for this host.
    #[allow(deprecated)]
    let rc = unsafe {
        libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            (&mut s as *mut libc::vm_statistics64).cast(),
            &mut count,
        )
    };
    if rc != 0 {
        return Err(io::Error::other(format!("host_statistics64: {rc}")));
    }
    Ok(VmStats {
        page_size: super::common::page_size(),
        free: s.free_count as u64,
        active: s.active_count as u64,
        inactive: s.inactive_count as u64,
        speculative: s.speculative_count as u64,
        throttled: s.throttled_count as u64,
        wired: s.wire_count as u64,
        purgeable: s.purgeable_count as u64,
        faults: s.faults,
        cow: s.cow_faults,
        zero_fill: s.zero_fill_count,
        reactivated: s.reactivations,
        purged: s.purges,
        file_backed: s.external_page_count as u64,
        anonymous: s.internal_page_count as u64,
        compressor_stored: s.total_uncompressed_pages_in_compressor,
        compressor_pages: s.compressor_page_count as u64,
        decompressions: s.decompressions,
        compressions: s.compressions,
        pageins: s.pageins,
        pageouts: s.pageouts,
        swapins: s.swapins,
        swapouts: s.swapouts,
    })
}

/// Cumulative ticks in the order the shared snapshot uses: user, nice, system, idle.
pub fn cpu_ticks() -> io::Result<[u64; 4]> {
    // SAFETY: zeroed host_cpu_load_info is a valid out-parameter for host_statistics.
    let mut s: libc::host_cpu_load_info = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: `s` is `count` integers wide.
    #[allow(deprecated)]
    let rc = unsafe {
        libc::host_statistics(
            libc::mach_host_self(),
            libc::HOST_CPU_LOAD_INFO,
            (&mut s as *mut libc::host_cpu_load_info).cast(),
            &mut count,
        )
    };
    if rc != 0 {
        return Err(io::Error::other(format!("host_statistics: {rc}")));
    }
    let t = s.cpu_ticks;
    Ok([
        t[libc::CPU_STATE_USER as usize] as u64,
        t[libc::CPU_STATE_NICE as usize] as u64,
        t[libc::CPU_STATE_SYSTEM as usize] as u64,
        t[libc::CPU_STATE_IDLE as usize] as u64,
    ])
}
