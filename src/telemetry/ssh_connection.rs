use std::{collections::HashSet, fs};

/// Identify the interactive OpenSSH transport. An ambiguous or shared socket has no safe fallback.
pub fn ports(pid: i32) -> Option<(u16, u16)> {
    if pid <= 0 {
        return None;
    }
    let mut sockets = HashSet::new();
    for fd in fs::read_dir(format!("/proc/{pid}/fd")).ok()?.flatten() {
        let Ok(link) = fs::read_link(fd.path()) else {
            continue;
        };
        let name = link.to_string_lossy();
        if let Some(inode) = name
            .strip_prefix("socket:[")
            .and_then(|s| s.strip_suffix(']'))
        {
            sockets.insert(inode.to_owned());
        }
    }
    let mut pairs = HashSet::new();
    for table in ["tcp", "tcp6"] {
        let Ok(contents) = fs::read_to_string(format!("/proc/{pid}/net/{table}")) else {
            continue;
        };
        for row in contents.lines().skip(1) {
            if let Some(pair) = socket_row(row, &sockets) {
                pairs.insert(pair);
            }
        }
    }
    (pairs.len() == 1).then(|| *pairs.iter().next().unwrap())
}

fn socket_row(row: &str, sockets: &HashSet<String>) -> Option<(u16, u16)> {
    let fields: Vec<_> = row.split_ascii_whitespace().collect();
    if fields.get(3) != Some(&"01") || !sockets.contains(*fields.get(9)?) {
        return None;
    }
    Some((port(fields[1])?, port(fields[2])?))
}

fn port(endpoint: &str) -> Option<u16> {
    u16::from_str_radix(endpoint.rsplit_once(':')?.1, 16).ok()
}
