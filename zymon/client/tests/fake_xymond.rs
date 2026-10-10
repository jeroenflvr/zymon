//! Loopback end-to-end: a fake xymond on 127.0.0.1:0, the real binary pointed at it.

mod common;
use common::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

const REPLY: &str = "clientversion:4.3.30\nlog:/var/log/syslog:10240\nfile:/etc/passwd\n";

/// Accepts `n` connections, reads each to EOF, answers `REPLY` to `client` messages only (like xymond).
fn fake_xymond(n: usize) -> (u16, mpsc::Receiver<Vec<u8>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..n {
            let Ok((mut c, _)) = l.accept() else { return };
            let mut got = Vec::new();
            c.read_to_end(&mut got).unwrap();
            if got.starts_with(b"client") {
                let _ = c.write_all(REPLY.as_bytes());
            }
            tx.send(got).unwrap();
        }
    });
    (port, rx)
}

fn config(dir: &std::path::Path, port: u16, extra_cfg: &str) -> std::path::PathBuf {
    let cfg = dir.join("c.toml");
    init(
        &cfg,
        &[
            "--server",
            &format!("127.0.0.1:{port}"),
            "--hostname",
            "my.host.example",
            "--os",
            "rhel",
            "--class",
            "mycls",
        ],
    );
    let body = std::fs::read_to_string(&cfg).unwrap();
    let state = dir.join("state");
    // top-level key first, then the generated tables, then optional extra tables
    std::fs::write(
        &cfg,
        format!(
            "state_dir = {:?}\n{body}\n{extra_cfg}",
            state.to_str().unwrap()
        ),
    )
    .unwrap();
    cfg
}

#[test]
fn client_message_reply_and_persistence() {
    let dir = tmpdir("e2e");
    let (port, rx) = fake_xymond(1);
    let cfg = config(&dir, port, "");
    let o = run(&["once", "--config", cfg.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

    let raw = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    let m = Msg::parse(&raw);
    // MACHINE has commas, OSTAG/CLASS are the configured overrides
    assert_eq!(m.first, "client my,host,example.rhel mycls");
    let order = expected_order();
    let idx: Vec<usize> = m
        .names()
        .iter()
        .map(|n| order.iter().position(|o| o == n).unwrap())
        .collect();
    assert!(
        idx.windows(2).all(|w| w[0] < w[1]),
        "sections out of order: {:?}",
        m.names()
    );
    let names = m.names();
    assert_eq!(&names[names.len() - 2..], ["clientversion", "clock"]);

    // reply persisted verbatim
    assert_eq!(
        std::fs::read_to_string(dir.join("state/clientcfg.txt")).unwrap(),
        REPLY
    );
}

#[test]
fn send_failure_exits_1_and_bad_config_exits_2() {
    let dir = tmpdir("fail");
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let cfg = config(&dir, port, "");
    let body = std::fs::read_to_string(&cfg)
        .unwrap()
        .replace("retries = 3", "retries = 1");
    std::fs::write(&cfg, body).unwrap();
    let o = run(&[
        "once",
        "--config",
        cfg.to_str().unwrap(),
        "--sections",
        "date,clock",
    ]);
    assert_eq!(o.status.code(), Some(1));

    let o = run(&[
        "once",
        "--config",
        dir.join("missing.toml").to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(2));
    let o = run(&[
        "once",
        "--config",
        cfg.to_str().unwrap(),
        "--sections",
        "bogus",
        "--dry-run",
    ]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn docker_must_run_sends_a_status_combo() {
    let dir = tmpdir("docker");
    let (port, rx) = fake_xymond(2);
    let cfg = config(&dir, port, "");
    let body = std::fs::read_to_string(&cfg).unwrap();
    // point docker at a socket that does not exist and require a container
    let body = body
        .replace(
            "socket = \"/var/run/docker.sock\"",
            "socket = \"/nonexistent/docker.sock\"",
        )
        .replace("must_run = []", "must_run = [\"web\"]");
    std::fs::write(&cfg, body).unwrap();
    let o = run(&["once", "--config", cfg.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let first = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert!(first.starts_with(b"client "));
    let second =
        String::from_utf8(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap()).unwrap();
    assert!(
        second.starts_with("combo\nstatus+10m my,host,example.docker red "),
        "{second}"
    );
    assert!(second.contains("&red cannot query docker"));
}

#[test]
fn init_refuses_to_overwrite() {
    let dir = tmpdir("init");
    let cfg = dir.join("c.toml");
    init(&cfg, &["--server", "127.0.0.1"]);
    let text = std::fs::read_to_string(&cfg).unwrap();
    // every wire section of this OS has a key (true/false), none missing
    for s in expected_order() {
        let present = text.lines().any(|l| l.starts_with(&format!("{s} = ")));
        let absent_ok = matches!(
            *s,
            "mdstat" | "docker" | "cputemp" | "ifconfig" | "osversion" | "crontab" | "who"
        );
        assert!(present || absent_ok, "missing key {s}");
    }
    let o = run(&[
        "init",
        "--config",
        cfg.to_str().unwrap(),
        "--server",
        "127.0.0.1",
    ]);
    assert_eq!(o.status.code(), Some(2));
    let o = run(&["init", "--config", cfg.to_str().unwrap(), "--force"]);
    assert!(o.status.success());
}

#[test]
fn version() {
    let o = run(&["version"]);
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("zymon-client "));
}
