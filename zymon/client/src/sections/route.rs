use super::{Ctx, Res};
use std::io::Write;
use zymon_core::proto::SectionWriter;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn ip4(hex: &str) -> Option<String> {
    let v = u32::from_str_radix(hex, 16).ok()?;
    let b = v.to_le_bytes();
    Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]))
}

/// /proc/net/route in `netstat -rn` layout.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn render(proc_route: &str) -> String {
    let mut s = String::from("Kernel IP routing table\n");
    s.push_str("Destination     Gateway         Genmask         Flags   MSS Window  irtt Iface\n");
    for l in proc_route.lines().skip(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 11 {
            continue;
        }
        let (Some(dst), Some(gw), Some(mask)) = (ip4(f[1]), ip4(f[2]), ip4(f[7])) else {
            continue;
        };
        let flags = u32::from_str_radix(f[3], 16).unwrap_or(0);
        let mut fl = String::new();
        for (bit, c) in [
            (0x1, 'U'),
            (0x2, 'G'),
            (0x4, 'H'),
            (0x8, 'R'),
            (0x10, 'D'),
            (0x20, 'M'),
            (0x200, '!'),
        ] {
            if flags & bit != 0 {
                fl.push(c);
            }
        }
        s.push_str(&format!(
            "{dst:<15} {gw:<15} {mask:<15} {fl:<5} {:>5} {:<6} {:>5} {}\n",
            f[8], f[9], f[10], f[0]
        ));
    }
    s
}

pub fn route(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    if cfg!(target_os = "macos") {
        let out = ctx.exec("netstat", &["-rn"])?;
        w.write_all(&out)
    } else {
        let t = crate::sys::linux::read("/proc/net/route")?;
        w.write_all(render(&t).as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netstat_layout() {
        let t = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\neth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\neth0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n";
        let r = render(t);
        let l: Vec<&str> = r.lines().collect();
        assert_eq!(l[0], "Kernel IP routing table");
        assert_eq!(
            l[2],
            "0.0.0.0         192.168.1.1     0.0.0.0         UG        0 0          0 eth0"
        );
        assert_eq!(
            l[3],
            "192.168.1.0     0.0.0.0         255.255.255.0   U         0 0          0 eth0"
        );
    }
}
