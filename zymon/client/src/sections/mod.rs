//! One function per section: `fn <name>(ctx, w)`. A failing section is skipped by the caller.

use crate::config::Config;
use crate::state::Snap;
use crate::sys::exec;
use std::io;
use zymon_core::proto::SectionWriter;

pub mod clientversion;
pub mod clock;
pub mod cpuinfo;
pub mod cputemp;
pub mod crontab;
pub mod date;
pub mod df;
pub mod docker;
pub mod free;
pub mod ifconfig;
pub mod ifstat;
pub mod inode;
pub mod mdstat;
pub mod meminfo;
pub mod mount;
pub mod netstat;
pub mod nproc;
pub mod osversion;
pub mod ports;
pub mod ps;
pub mod route;
pub mod smart;
pub mod top;
pub mod uname;
pub mod uptime;
pub mod vmstat;
pub mod who;

pub type Res = io::Result<()>;

/// Wire order on Linux; `[clientversion]` and `[clock]` are always last, in that order.
pub const LINUX: &[&str] = &[
    "date",
    "uname",
    "osversion",
    "uptime",
    "who",
    "df",
    "inode",
    "mount",
    "free",
    "ifconfig",
    "route",
    "netstat",
    "ports",
    "ifstat",
    "mdstat",
    "smart",
    "ps",
    "nproc",
    "top",
    "vmstat",
    "cpuinfo",
    "cputemp",
    "crontab",
    "docker",
    "clientversion",
    "clock",
];

pub const DARWIN: &[&str] = &[
    "date",
    "uname",
    "uptime",
    "who",
    "df",
    "inode",
    "mount",
    "meminfo",
    "ifconfig",
    "route",
    "netstat",
    "ifstat",
    "ports",
    "smart",
    "ps",
    "nproc",
    "top",
    "cpuinfo",
    "crontab",
    "docker",
    "clientversion",
    "clock",
];

pub fn order() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        DARWIN
    } else {
        LINUX
    }
}

/// Per-cycle context: config, counter snapshots and captures shared between sections.
pub struct Ctx<'a> {
    pub cfg: &'a Config,
    pub prev: Option<Snap>,
    pub cur: Option<Snap>,
    /// Raw `ps` capture, reused by `[top]` (`ps` always runs before `top`).
    pub ps: Option<Vec<u8>>,
    /// `[smart]` capture, handed to the `smart` status column after the client message is built.
    pub smart: Option<Vec<smart::Disk>>,
    who: Option<Vec<u8>>,
}

impl<'a> Ctx<'a> {
    pub fn new(cfg: &'a Config, prev: Option<Snap>, cur: Option<Snap>) -> Self {
        Ctx {
            cfg,
            prev,
            cur,
            ps: None,
            smart: None,
            who: None,
        }
    }

    pub fn exec(&self, cmd: &str, args: &[&str]) -> io::Result<Vec<u8>> {
        exec::run_ok(cmd, args, self.cfg.exec_timeout())
    }

    /// `who` output, run once per cycle (feeds `[who]` and the user count in `[uptime]`/`[top]`).
    pub fn who(&mut self) -> &[u8] {
        if self.who.is_none() {
            self.who = Some(self.exec("who", &[]).unwrap_or_default());
        }
        self.who.as_deref().unwrap_or_default()
    }

    pub fn users(&mut self) -> usize {
        self.who()
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .count()
    }
}

pub fn run(name: &str, ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    match name {
        "date" => date::date(ctx, w),
        "uname" => uname::uname(ctx, w),
        "osversion" => osversion::osversion(ctx, w),
        "uptime" => uptime::uptime(ctx, w),
        "who" => who::who(ctx, w),
        "df" => df::df(ctx, w),
        "inode" => inode::inode(ctx, w),
        "mount" => mount::mount(ctx, w),
        "free" => free::free(ctx, w),
        "meminfo" => meminfo::meminfo(ctx, w),
        "ifconfig" => ifconfig::ifconfig(ctx, w),
        "route" => route::route(ctx, w),
        "netstat" => netstat::netstat(ctx, w),
        "ports" => ports::ports(ctx, w),
        "ifstat" => ifstat::ifstat(ctx, w),
        "mdstat" => mdstat::mdstat(ctx, w),
        "smart" => smart::smart(ctx, w),
        "ps" => ps::ps(ctx, w),
        "nproc" => nproc::nproc(ctx, w),
        "top" => top::top(ctx, w),
        "vmstat" => vmstat::vmstat(ctx, w),
        "cpuinfo" => cpuinfo::cpuinfo(ctx, w),
        "cputemp" => cputemp::cputemp(ctx, w),
        "crontab" => crontab::crontab(ctx, w),
        "docker" => docker::docker(ctx, w),
        "clientversion" => clientversion::clientversion(ctx, w),
        "clock" => clock::clock(ctx, w),
        _ => Err(io::Error::other(format!("unknown section {name}"))),
    }
}

/// Sections that exist on this OS but have nothing to do (e.g. Linux-only on macOS) report this.
pub fn unsupported() -> Res {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "not available on this OS",
    ))
}
