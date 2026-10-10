use super::{Ctx, Res};
use std::io::Write;
use zymon_core::proto::SectionWriter;

/// Exactly the legacy scripts' column lists; the server finds `CMD`/`COMMAND` in the header line.
#[cfg(not(target_os = "macos"))]
const ARGS: &[&str] = &[
    "-Aww",
    "f",
    "-o",
    "pid,ppid,user,start,state,pri,pcpu,time:12,pmem,rsz:10,vsz:10,cmd",
];
#[cfg(target_os = "macos")]
const ARGS: &[&str] = &[
    "-ax",
    "-ww",
    "-o",
    "pid,ppid,user,start,state,pri,pcpu,time,pmem,rss,vsz,command",
];

pub fn ps(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    let out = ctx.exec("ps", ARGS)?;
    w.write_all(&out)?;
    ctx.ps = Some(out);
    Ok(())
}

pub struct Row {
    pub pid: String,
    pub user: String,
    pub pri: String,
    pub state: String,
    pub pcpu: f64,
    pub time: String,
    pub pmem: String,
    pub rss: String,
    pub vsz: String,
    pub cmd: String,
}

/// Parse the capture above (header line skipped). Columns: pid ppid user start state pri pcpu time pmem rss vsz cmd...
/// `start` may be "Oct10" or "10:01" (one token) on both platforms.
pub fn parse(capture: &[u8]) -> Vec<Row> {
    let text = String::from_utf8_lossy(capture);
    let mut rows = Vec::new();
    for l in text.lines().skip(1) {
        let mut it = l.split_whitespace();
        let mut take = |n: usize| -> Option<Vec<&str>> { (0..n).map(|_| it.next()).collect() };
        let Some(f) = take(11) else { continue };
        // the command is everything after the 11th token, including forest art and spaces
        let rest = rest_after(l, 11);
        rows.push(Row {
            pid: f[0].into(),
            user: f[2].into(),
            state: f[4].into(),
            pri: f[5].into(),
            pcpu: f[6].parse().unwrap_or(0.0),
            time: f[7].into(),
            pmem: f[8].into(),
            rss: f[9].into(),
            vsz: f[10].into(),
            cmd: strip_forest(rest).to_string(),
        });
    }
    rows
}

fn rest_after(line: &str, n: usize) -> &str {
    let mut s = line.trim_start();
    for _ in 0..n {
        s = s
            .trim_start_matches(|c: char| !c.is_whitespace())
            .trim_start();
    }
    s
}

fn strip_forest(mut s: &str) -> &str {
    loop {
        let t = s.trim_start();
        if let Some(r) = t.strip_prefix("\\_ ").or_else(|| t.strip_prefix('|')) {
            s = r;
        } else {
            return t;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_linux_forest() {
        let cap = b"  PID  PPID USER      STARTED S PRI %CPU         TIME %MEM      RSZ        VSZ CMD\n    1     0 root      Oct 09 S  19  0.1     00:00:03  0.5     4000      16000 /sbin/init\n  300     1 root       10:00 S  19  1.5     00:01:00  1.0     8000      30000  \\_ /usr/sbin/sshd -D\n";
        // `start` as two tokens would shift columns; the real header uses STARTED with "Oct09"/"10:00" tokens.
        let cap = String::from_utf8_lossy(cap).replace("Oct 09", "Oct09");
        let rows = parse(cap.as_bytes());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].cmd, "/sbin/init");
        assert_eq!(rows[1].cmd, "/usr/sbin/sshd -D");
        assert_eq!(rows[1].pcpu, 1.5);
        assert_eq!(rows[1].rss, "8000");
    }
}
