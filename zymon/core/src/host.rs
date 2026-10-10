//! Pure naming helpers for the `client MACHINE.OSTAG CLASS` header.

/// Legacy servers split the OS tag off at the last `.`, so dots in the hostname go on the wire as commas.
pub fn machine(hostname: &str) -> String {
    hostname.replace('.', ",")
}

/// Lowercase `uname -s` with `/` -> `_` (`Linux` -> `linux`, `HP-UX` -> `hp-ux`, `OS/390` -> `os_390`).
pub fn os_tag(sysname: &str) -> String {
    sysname.trim().to_ascii_lowercase().replace('/', "_")
}

/// The class defaults to the OS tag.
pub fn class_or_default<'a>(class: &'a str, os: &'a str) -> &'a str {
    if class.is_empty() { os } else { class }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commafy() {
        assert_eq!(machine("pi4.lan.example.org"), "pi4,lan,example,org");
        assert_eq!(machine("pi4"), "pi4");
    }

    #[test]
    fn tags() {
        assert_eq!(os_tag("Linux\n"), "linux");
        assert_eq!(os_tag("Darwin"), "darwin");
        assert_eq!(os_tag("OS/390"), "os_390");
        assert_eq!(class_or_default("", "linux"), "linux");
        assert_eq!(class_or_default("rpi", "linux"), "rpi");
    }
}
