//! libc shims shared by Linux and macOS.

use std::ffi::{CStr, CString};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

pub struct Uname {
    pub sysname: String,
    pub nodename: String,
    pub release: String,
    pub version: String,
    pub machine: String,
}

fn cstr_field(f: &[libc::c_char]) -> String {
    // SAFETY: utsname fields are NUL-terminated within their fixed-size arrays.
    unsafe { CStr::from_ptr(f.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

pub fn uname() -> io::Result<Uname> {
    // SAFETY: zeroed utsname is a valid out-parameter for uname(2).
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: `u` is a valid, writable utsname.
    if unsafe { libc::uname(&mut u) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Uname {
        sysname: cstr_field(&u.sysname),
        nodename: cstr_field(&u.nodename),
        release: cstr_field(&u.release),
        version: cstr_field(&u.version),
        machine: cstr_field(&u.machine),
    })
}

pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: buffer is valid for `buf.len()` bytes; we force a NUL terminator afterwards.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len() - 1) };
    if rc != 0 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

pub fn loadavg() -> Option<[f64; 3]> {
    let mut l = [0f64; 3];
    // SAFETY: `l` has room for the 3 samples requested.
    let n = unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
    (n == 3).then_some(l)
}

pub fn nproc() -> usize {
    // SAFETY: sysconf has no memory-safety preconditions.
    let n = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) };
    n.max(1) as usize
}

pub fn page_size() -> u64 {
    // SAFETY: sysconf has no memory-safety preconditions.
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if n > 0 { n as u64 } else { 4096 }
}

pub fn euid() -> u32 {
    // SAFETY: geteuid cannot fail.
    unsafe { libc::geteuid() }
}

/// `(seconds, microseconds)` since the epoch.
pub fn now() -> (i64, u32) {
    // SAFETY: zeroed timespec is a valid out-parameter.
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    // SAFETY: `ts` is valid and writable.
    unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts) };
    (ts.tv_sec as i64, (ts.tv_nsec / 1000) as u32)
}

/// strftime(3) of epoch `t`, in local time or UTC.
pub fn strftime(fmt: &str, t: i64, utc: bool) -> String {
    let Ok(cfmt) = CString::new(fmt) else {
        return String::new();
    };
    let tt = t as _; // time_t: width differs per target
    // SAFETY: zeroed tm is a valid out-parameter for the *_r functions.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `tt` and `tm` are valid for the duration of the call.
    let ok = unsafe {
        if utc {
            !libc::gmtime_r(&tt, &mut tm).is_null()
        } else {
            !libc::localtime_r(&tt, &mut tm).is_null()
        }
    };
    if !ok {
        return String::new();
    }
    let mut buf = [0u8; 128];
    // SAFETY: buffer length is passed; format is NUL-terminated; tm was filled above.
    let n = unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), cfmt.as_ptr(), &tm) };
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

#[derive(Default, Clone, Copy)]
pub struct StatVfs {
    pub frsize: u64,
    pub blocks: u64,
    pub bfree: u64,
    pub bavail: u64,
    pub files: u64,
    pub ffree: u64,
}

#[allow(clippy::unnecessary_cast)] // field widths differ per target
pub fn statvfs(path: &str) -> Option<StatVfs> {
    let c = CString::new(path).ok()?;
    // SAFETY: zeroed statvfs is a valid out-parameter.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is NUL-terminated and `s` is writable.
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    Some(StatVfs {
        frsize: if s.f_frsize != 0 {
            s.f_frsize
        } else {
            s.f_bsize
        } as u64,
        blocks: s.f_blocks as u64,
        bfree: s.f_bfree as u64,
        bavail: s.f_bavail as u64,
        files: s.f_files as u64,
        ffree: s.f_ffree as u64,
    })
}

/// True when the current user may read and write `path` (enough to connect to a unix socket).
pub fn can_rw(path: &str) -> bool {
    let Ok(c) = CString::new(path) else {
        return false;
    };
    // SAFETY: `c` is NUL-terminated.
    unsafe { libc::access(c.as_ptr(), libc::R_OK | libc::W_OK) == 0 }
}

static STOP: AtomicBool = AtomicBool::new(false);
static RELOAD: AtomicBool = AtomicBool::new(false);
static PIPE_W: AtomicI32 = AtomicI32::new(-1);
static PIPE_R: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(sig: libc::c_int) {
    if sig == libc::SIGHUP {
        RELOAD.store(true, Ordering::SeqCst);
    } else {
        STOP.store(true, Ordering::SeqCst);
    }
    let fd = PIPE_W.load(Ordering::Relaxed);
    if fd >= 0 {
        let b = 1u8;
        // SAFETY: write(2) is async-signal-safe; the buffer is a valid 1-byte local.
        unsafe { libc::write(fd, (&b as *const u8).cast(), 1) };
    }
}

fn set_nonblock_cloexec(fd: libc::c_int) {
    // SAFETY: fcntl on a descriptor we own.
    unsafe {
        let fl = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }
}

/// SIGTERM/SIGINT request a stop, SIGHUP a config reload. A self-pipe wakes `wait_for_event`
/// race-free, so a signal arriving just before the sleep still ends it immediately.
pub fn install_signals() -> io::Result<()> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `fds` has room for the two descriptors.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    set_nonblock_cloexec(fds[0]);
    set_nonblock_cloexec(fds[1]);
    PIPE_R.store(fds[0], Ordering::SeqCst);
    PIPE_W.store(fds[1], Ordering::SeqCst);
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: the handler only touches atomics and calls write(2).
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as *const () as usize;
            libc::sigemptyset(&mut sa.sa_mask);
            sa.sa_flags = 0;
            if libc::sigaction(sig, &sa, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

pub fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}

pub fn take_reload() -> bool {
    RELOAD.swap(false, Ordering::SeqCst)
}

/// Sleep up to `d`, returning early when a signal arrives.
pub fn wait_for_event(d: Duration) {
    let fd = PIPE_R.load(Ordering::SeqCst);
    if stop_requested() || fd < 0 {
        return;
    }
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let ms = d.as_millis().min(i32::MAX as u128) as libc::c_int;
    // SAFETY: one valid pollfd.
    unsafe { libc::poll(&mut p, 1, ms) };
    let mut b = [0u8; 16];
    // SAFETY: non-blocking drain into a local buffer.
    while unsafe { libc::read(fd, b.as_mut_ptr().cast(), b.len()) } > 0 {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_formats() {
        assert_eq!(
            strftime("%Y-%m-%d %H:%M:%S", 0, true),
            "1970-01-01 00:00:00"
        );
        assert_eq!(strftime("%a %b %e", 86400 * 40, true), "Tue Feb 10");
    }

    #[test]
    fn basics() {
        assert!(!hostname().is_empty());
        assert!(nproc() >= 1);
        assert!(statvfs("/").is_some());
        assert!(uname().unwrap().sysname.len() > 2);
    }
}
