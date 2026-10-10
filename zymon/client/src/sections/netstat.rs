use super::{Ctx, Res};
use crate::sys::exec;
use std::collections::HashMap;
use std::io::Write;
use zymon_core::proto::SectionWriter;

/// `netstat -s` wording for the counters the legacy RRD handler reads (do_netstat.c, "segments sent out"
/// flavour). The `Ip:` block must come first: the handler skips to "\nTcp:".
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn render_snmp(snmp: &str) -> String {
    let mut tables: HashMap<&str, HashMap<&str, u64>> = HashMap::new();
    let mut lines = snmp.lines();
    while let (Some(names), Some(vals)) = (lines.next(), lines.next()) {
        let (Some((proto, names)), Some((_, vals))) = (names.split_once(':'), vals.split_once(':'))
        else {
            continue;
        };
        let m = tables.entry(proto).or_default();
        for (n, v) in names.split_whitespace().zip(vals.split_whitespace()) {
            m.insert(n, v.parse().unwrap_or(0));
        }
    }
    let g = |p: &str, k: &str| tables.get(p).and_then(|m| m.get(k)).copied().unwrap_or(0);
    let mut s = String::from("Ip:\n");
    s.push_str(&format!("    Forwarding: {}\n", g("Ip", "Forwarding")));
    s.push_str(&format!(
        "    {} total packets received\n",
        g("Ip", "InReceives")
    ));
    s.push_str(&format!(
        "    {} incoming packets delivered\n",
        g("Ip", "InDelivers")
    ));
    s.push_str(&format!(
        "    {} requests sent out\n",
        g("Ip", "OutRequests")
    ));
    s.push_str("Tcp:\n");
    s.push_str(&format!(
        "    {} active connection openings\n",
        g("Tcp", "ActiveOpens")
    ));
    s.push_str(&format!(
        "    {} passive connection openings\n",
        g("Tcp", "PassiveOpens")
    ));
    s.push_str(&format!(
        "    {} failed connection attempts\n",
        g("Tcp", "AttemptFails")
    ));
    s.push_str(&format!(
        "    {} connection resets received\n",
        g("Tcp", "EstabResets")
    ));
    s.push_str(&format!(
        "    {} connections established\n",
        g("Tcp", "CurrEstab")
    ));
    s.push_str(&format!("    {} segments received\n", g("Tcp", "InSegs")));
    s.push_str(&format!("    {} segments sent out\n", g("Tcp", "OutSegs")));
    s.push_str(&format!(
        "    {} segments retransmitted\n",
        g("Tcp", "RetransSegs")
    ));
    s.push_str(&format!(
        "    {} bad segments received\n",
        g("Tcp", "InErrs")
    ));
    s.push_str(&format!("    {} resets sent\n", g("Tcp", "OutRsts")));
    s.push_str("Udp:\n");
    s.push_str(&format!(
        "    {} packets received\n",
        g("Udp", "InDatagrams")
    ));
    s.push_str(&format!(
        "    {} packets to unknown port received\n",
        g("Udp", "NoPorts")
    ));
    s.push_str(&format!(
        "    {} packet receive errors\n",
        g("Udp", "InErrors")
    ));
    s.push_str(&format!("    {} packets sent\n", g("Udp", "OutDatagrams")));
    s
}

/// The real `netstat -s` when installed (legacy output verbatim); on Linux without net-tools,
/// the same counters synthesised from /proc/net/snmp.
pub fn netstat(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    if cfg!(target_os = "macos") || exec::which("netstat").is_some() {
        let out = ctx.exec("netstat", &["-s"])?;
        return w.write_all(&out);
    }
    let t = crate::sys::linux::read("/proc/net/snmp")?;
    w.write_all(render_snmp(&t).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNMP: &str = "Ip: Forwarding DefaultTTL InReceives InDelivers OutRequests\nIp: 2 64 5000 4900 3000\nTcp: RtoAlgorithm RtoMin RtoMax MaxConn ActiveOpens PassiveOpens AttemptFails EstabResets CurrEstab InSegs OutSegs RetransSegs InErrs OutRsts InCsumErrors\nTcp: 1 200 120000 -1 11 22 3 4 5 666 777 8 9 10 0\nUdp: InDatagrams NoPorts InErrors OutDatagrams RcvbufErrors SndbufErrors InCsumErrors IgnoredMulti\nUdp: 100 2 3 200 0 0 0 0\n";

    /// Port of do_valbeforemarker() with the "segments sent out" marker set.
    fn value_before(msg: &str, marker: &str) -> Option<u64> {
        let at = msg.find(marker)?;
        let bol = msg[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let digits: String = msg[bol..]
            .trim_start()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    }

    #[test]
    fn legacy_rrd_markers_resolve() {
        let t = render_snmp(SNMP);
        let tcp = &t[t.find("\nTcp:").unwrap()..];
        assert!(tcp.contains("segments sent out"));
        let want = [
            ("packets received", 100),
            ("packets sent", 200),
            ("packet receive errors", 3),
            ("active connection openings", 11),
            ("passive connection openings", 22),
            ("failed connection attempts", 3),
            ("connection resets received", 4),
            ("connections established", 5),
            ("segments sent out", 777),
            ("segments received", 666),
            ("segments retransmitted", 8),
        ];
        for (m, v) in want {
            assert_eq!(value_before(tcp, m), Some(v), "marker {m}");
        }
    }
}
