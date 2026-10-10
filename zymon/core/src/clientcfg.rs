//! The reply to a `client` message is a logfetch.cfg-style config. Phase 1 only parses and persists it.

#[derive(Debug, PartialEq, Eq)]
pub enum Entry {
    /// `clientversion:X` - zymon never self-updates, callers ignore it.
    ClientVersion(String),
    /// `log:/path:maxbytes`
    Log(String),
    /// `file:/path`
    File(String),
    /// `dir:/path`
    Dir(String),
    /// `linecount:`, `ignore`, `trigger`, and everything else the logfetch port will need later.
    Other(String),
}

pub fn parse(reply: &[u8]) -> Vec<Entry> {
    let mut out = Vec::new();
    for raw in reply.split(|&b| b == b'\n') {
        let line = String::from_utf8_lossy(raw);
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let entry = if let Some(v) = line.strip_prefix("clientversion:") {
            Entry::ClientVersion(v.trim().to_string())
        } else if let Some(v) = line.strip_prefix("log:") {
            Entry::Log(v.to_string())
        } else if let Some(v) = line.strip_prefix("file:") {
            Entry::File(v.to_string())
        } else if let Some(v) = line.strip_prefix("dir:") {
            Entry::Dir(v.to_string())
        } else {
            Entry::Other(line.to_string())
        };
        out.push(entry);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reply() {
        let reply = b"clientversion:4.3.30\nlog:/var/log/messages:10240\nignore MARK\n\nfile:/etc/passwd\ndir:/tmp\n";
        let e = parse(reply);
        assert_eq!(e.len(), 5);
        assert_eq!(e[0], Entry::ClientVersion("4.3.30".into()));
        assert_eq!(e[1], Entry::Log("/var/log/messages:10240".into()));
        assert_eq!(e[2], Entry::Other("ignore MARK".into()));
        assert_eq!(e[3], Entry::File("/etc/passwd".into()));
        assert_eq!(e[4], Entry::Dir("/tmp".into()));
    }

    #[test]
    fn empty_reply() {
        assert!(parse(b"").is_empty());
    }
}
