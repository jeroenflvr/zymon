use super::{Ctx, Res};
use zymon_core::proto::SectionWriter;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn ip4(hex: &str) -> Option<String> {
    let v = u32::from_str_radix(hex, 16).ok()?;
    let b = v.to_le_bytes();
    Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]))
}

/// 32 hex digits = four host-order (little-endian) words.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn ip6(hex: &str) -> Option<String> {
    if hex.len() != 32 {
        return None;
    }
    let mut b = [0u8; 16];
    for w in 0..4 {
        let v = u32::from_str_radix(&hex[w * 8..w * 8 + 8], 16).ok()?;
        b[w * 4..w * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    let g: Vec<u16> = (0..8)
        .map(|i| u16::from_be_bytes([b[i * 2], b[i * 2 + 1]]))
        .collect();
    if g[..5] == [0; 5] && g[5] == 0xffff {
        return Some(format!("::ffff:{}.{}.{}.{}", b[12], b[13], b[14], b[15]));
    }
    // compress the longest run (>= 2) of zero groups
    let (mut best, mut best_len, mut i) = (0, 0, 0);
    while i < 8 {
        if g[i] == 0 {
            let s = i;
            while i < 8 && g[i] == 0 {
                i += 1;
            }
            if i - s > best_len {
                best = s;
                best_len = i - s;
            }
        } else {
            i += 1;
        }
    }
    let hexs = |r: &[u16]| {
        r.iter()
            .map(|x| format!("{x:x}"))
            .collect::<Vec<_>>()
            .join(":")
    };
    Some(if best_len >= 2 {
        format!("{}::{}", hexs(&g[..best]), hexs(&g[best + best_len..]))
    } else {
        hexs(&g)
    })
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn endpoint(s: &str, v6: bool) -> Option<String> {
    let (a, p) = s.split_once(':')?;
    let addr = if v6 { ip6(a)? } else { ip4(a)? };
    let port = u16::from_str_radix(p, 16).ok()?;
    Some(format!("{addr}:{port}"))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn tcp_state(st: &str) -> &'static str {
    match st {
        "01" => "ESTABLISHED",
        "02" => "SYN_SENT",
        "03" => "SYN_RECV",
        "04" => "FIN_WAIT1",
        "05" => "FIN_WAIT2",
        "06" => "TIME_WAIT",
        "07" => "CLOSE",
        "08" => "CLOSE_WAIT",
        "09" => "LAST_ACK",
        "0A" => "LISTEN",
        "0B" => "CLOSING",
        _ => "UNKNOWN",
    }
}

/// Append the rows of one /proc/net/{tcp,tcp6,udp,udp6} file in `netstat -antuW` layout.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn render_proto(out: &mut String, proto: &str, text: &str) {
    let v6 = proto.ends_with('6');
    let udp = proto.starts_with("udp");
    for l in text.lines().skip(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 5 {
            continue;
        }
        let (Some(local), Some(remote)) = (endpoint(f[1], v6), endpoint(f[2], v6)) else {
            continue;
        };
        let remote = match remote.rsplit_once(':') {
            Some((a, "0")) => format!("{a}:*"),
            _ => remote,
        };
        let (tx, rx) = f[4].split_once(':').unwrap_or(("0", "0"));
        let tx = u64::from_str_radix(tx, 16).unwrap_or(0);
        let rx = u64::from_str_radix(rx, 16).unwrap_or(0);
        let state = if udp {
            match f[3] {
                "01" => "ESTABLISHED",
                _ => "",
            }
        } else {
            tcp_state(&f[3].to_ascii_uppercase())
        };
        let row = format!("{proto:<5} {rx:>6} {tx:>6} {local:<23} {remote:<23} {state}");
        out.push_str(row.trim_end());
        out.push('\n');
    }
}

#[cfg(target_os = "macos")]
fn filter_mac(netstat_an: &[u8], w: &mut SectionWriter) -> Res {
    for l in netstat_an.split(|&b| b == b'\n') {
        if l.starts_with(b"tcp") || l.starts_with(b"udp") {
            w.line(l);
        }
    }
    Ok(())
}

pub fn ports(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    #[cfg(target_os = "macos")]
    {
        let out = ctx.exec("netstat", &["-an"])?;
        filter_mac(&out, w)
    }
    #[cfg(not(target_os = "macos"))]
    {
        use std::io::Write;
        let _ = ctx;
        let mut s = String::from("Active Internet connections (servers and established)\n");
        s.push_str("Proto Recv-Q Send-Q Local Address           Foreign Address         State\n");
        let mut any = false;
        for (proto, path) in [
            ("tcp", "/proc/net/tcp"),
            ("tcp6", "/proc/net/tcp6"),
            ("udp", "/proc/net/udp"),
            ("udp6", "/proc/net/udp6"),
        ] {
            // IPv6 may be compiled out; one readable file is enough.
            if let Ok(t) = crate::sys::linux::read(path) {
                any = true;
                render_proto(&mut s, proto, &t);
            }
        }
        if !any {
            return Err(std::io::Error::other("no /proc/net/{tcp,udp}"));
        }
        w.write_all(s.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp4_rows() {
        let t = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0 100 0 0 10 0\n   1: 0A01A8C0:0016 0201A8C0:C350 01 00000010:00000005 00:00000000 00000000     0        0 1235 1 0 20 4 28 10 -1\n";
        let mut s = String::new();
        render_proto(&mut s, "tcp", t);
        let l: Vec<Vec<&str>> = s.lines().map(|l| l.split_whitespace().collect()).collect();
        // legacy columns: 3=local 4=remote 5=state
        assert_eq!(l[0][3], "0.0.0.0:22");
        assert_eq!(l[0][4], "0.0.0.0:*");
        assert_eq!(l[0][5], "LISTEN");
        assert_eq!(l[1][1], "5");
        assert_eq!(l[1][2], "16");
        assert_eq!(l[1][3], "192.168.1.10:22");
        assert_eq!(l[1][4], "192.168.1.2:50000");
        assert_eq!(l[1][5], "ESTABLISHED");
    }

    #[test]
    fn ipv6_formats() {
        assert_eq!(ip6("00000000000000000000000000000000").unwrap(), "::");
        assert_eq!(ip6("00000000000000000000000001000000").unwrap(), "::1");
        assert_eq!(
            ip6("0000000000000000FFFF00000100007F").unwrap(),
            "::ffff:127.0.0.1"
        );
        assert_eq!(
            ip6("B80D0120000000000000000001000000").unwrap(),
            "2001:db8::1"
        );
        let mut s = String::new();
        render_proto(
            &mut s,
            "tcp6",
            "h\n   0: 00000000000000000000000000000000:0016 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000 0 0 1 1 0 0 0 0\n",
        );
        assert!(s.starts_with("tcp6 "));
        assert!(s.contains(":::22 "));
        assert!(s.contains(":::*"));
    }

    #[test]
    fn udp_has_no_state_when_unconnected() {
        let mut s = String::new();
        render_proto(
            &mut s,
            "udp",
            "h\n  10: 00000000:0044 00000000:0000 07 00000000:00000000 00:00000000 00000000 0 0 1 2 0\n",
        );
        let c: Vec<&str> = s.split_whitespace().collect();
        assert_eq!(c.len(), 5);
        assert_eq!(c[3], "0.0.0.0:68");
    }
}
