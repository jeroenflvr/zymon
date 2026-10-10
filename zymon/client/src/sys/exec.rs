//! Run an external command with a hard timeout. Fixed PATH, LANG=C, no stdin.

use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(not(target_os = "macos"))]
pub const SEARCH_PATH: &[&str] = &[
    "/usr/local/sbin",
    "/usr/local/bin",
    "/usr/sbin",
    "/usr/bin",
    "/sbin",
    "/bin",
];

/// Homebrew (Apple silicon) first: that is where smartctl lives on a Mac.
#[cfg(target_os = "macos")]
pub const SEARCH_PATH: &[&str] = &[
    "/opt/homebrew/sbin",
    "/opt/homebrew/bin",
    "/usr/local/sbin",
    "/usr/local/bin",
    "/usr/sbin",
    "/usr/bin",
    "/sbin",
    "/bin",
];

pub struct Output {
    pub stdout: Vec<u8>,
    pub success: bool,
}

pub fn which(cmd: &str) -> Option<PathBuf> {
    SEARCH_PATH
        .iter()
        .map(|d| PathBuf::from(d).join(cmd))
        .find(|p| {
            std::fs::metadata(p)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

pub fn run(cmd: &str, args: &[&str], timeout: Duration) -> io::Result<Output> {
    let path = which(cmd)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{cmd}: not found")))?;
    let mut child = Command::new(path)
        .args(args)
        .env_clear()
        .env("PATH", SEARCH_PATH.join(":"))
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut out = child.stdout.take().expect("piped stdout");
    let fd = out.as_raw_fd();
    let deadline = Instant::now() + timeout;
    let mut buf = Vec::with_capacity(16 * 1024);
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{cmd}: timed out"),
            ));
        }
        let mut p = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = left.as_millis().min(i32::MAX as u128) as libc::c_int;
        // SAFETY: one valid pollfd for a descriptor we own.
        let rc = unsafe { libc::poll(&mut p, 1, ms.max(1)) };
        if rc < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        if rc == 0 {
            continue; // re-check the deadline
        }
        match out.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        }
    }
    let status = child.wait()?;
    Ok(Output {
        stdout: buf,
        success: status.success(),
    })
}

/// Like `run`, but a failing exit status with no output is an error.
pub fn run_ok(cmd: &str, args: &[&str], timeout: Duration) -> io::Result<Vec<u8>> {
    let o = run(cmd, args, timeout)?;
    if !o.success && o.stdout.is_empty() {
        return Err(io::Error::other(format!("{cmd} failed")));
    }
    Ok(o.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_and_captures() {
        let o = run("echo", &["hi"], Duration::from_secs(5)).unwrap();
        assert_eq!(o.stdout, b"hi\n");
        assert!(o.success);
    }

    #[test]
    fn kills_on_timeout() {
        let t = Instant::now();
        let e = run("sleep", &["10"], Duration::from_millis(200))
            .err()
            .unwrap();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(t.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn missing_command() {
        assert_eq!(
            run("no-such-cmd-zymon", &[], Duration::from_secs(1))
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::NotFound
        );
    }
}
