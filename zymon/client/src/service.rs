//! `zymon-client restart`: ask the service manager (systemd / launchd) to restart the installed service.

use crate::sys::{common, exec};
use std::os::unix::process::CommandExt;
use std::process::Command;

pub const LAUNCHD_LABEL: &str = "org.zymon.client";
pub const SYSTEMD_UNIT: &str = "zymon-client";

/// The command line that restarts the service; `sudo` in front when not root.
pub fn restart_argv(mac: bool, root: bool, have_sudo: bool) -> Result<Vec<String>, String> {
    let mut v: Vec<String> = Vec::new();
    if !root {
        if !have_sudo {
            return Err("restarting the service needs root: run as root or install sudo".into());
        }
        v.push("sudo".into());
    }
    if mac {
        v.extend(["launchctl", "kickstart", "-k"].map(String::from));
        v.push(format!("system/{LAUNCHD_LABEL}"));
    } else {
        v.extend(["systemctl", "restart"].map(String::from));
        v.push(SYSTEMD_UNIT.into());
    }
    Ok(v)
}

/// Replaces this process with the service manager command; only returns on failure.
pub fn restart() -> String {
    let mac = cfg!(target_os = "macos");
    let tool = if mac { "launchctl" } else { "systemctl" };
    if exec::which(tool).is_none() {
        return format!(
            "{tool} not found: no service manager to restart the client with (start it with `zymon-client run`)"
        );
    }
    let argv = match restart_argv(mac, common::euid() == 0, exec::which("sudo").is_some()) {
        Ok(a) => a,
        Err(e) => return e,
    };
    let err = Command::new(&argv[0]).args(&argv[1..]).exec();
    format!("cannot run {}: {err}", argv.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        assert_eq!(
            restart_argv(false, true, false).unwrap(),
            ["systemctl", "restart", "zymon-client"]
        );
        assert_eq!(
            restart_argv(false, false, true).unwrap(),
            ["sudo", "systemctl", "restart", "zymon-client"]
        );
        assert_eq!(
            restart_argv(true, false, true).unwrap(),
            [
                "sudo",
                "launchctl",
                "kickstart",
                "-k",
                "system/org.zymon.client"
            ]
        );
        assert!(restart_argv(true, false, false).is_err());
    }
}
