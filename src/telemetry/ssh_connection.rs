//! Identify the active SSH transport and replay its connection options.
use crate::tabs::is_ssh_host;
use crate::{proc_radar, ssh_detect};
use std::{collections::HashSet, fs};
use std::{
    io,
    path::PathBuf,
    process::{Command, Stdio},
};

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

const SSH_OPTIONS: &[&str] = &[
    "-T",
    "-oBatchMode=yes",
    "-oConnectTimeout=6",
    "-oClearAllForwardings=yes",
    "-oPermitLocalCommand=no",
    "-oRemoteCommand=none",
    "-oForwardAgent=no",
    "-oForwardX11=no",
    "-oControlMaster=no",
    "-oStrictHostKeyChecking=yes",
];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SshRoute {
    args: Vec<String>,
    cwd: PathBuf,
}

impl SshRoute {
    pub fn from_client(pid: i32, host: &str) -> Option<Self> {
        let argv = proc_radar::read_proc_cmdline(pid)?;
        if ssh_detect::parse_ssh_argv(&argv)?.as_str() != host {
            return None;
        }
        let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
        Self::from_argv(&argv, cwd)
    }

    pub(crate) fn from_argv(argv: &[String], cwd: PathBuf) -> Option<Self> {
        Some(Self {
            args: connection_args(argv)?,
            cwd,
        })
    }

    pub(crate) fn for_host(host: &str) -> Option<Self> {
        if !is_ssh_host(host) {
            return None;
        }
        let args = vec!["--".to_owned(), host.to_owned()];
        Some(Self {
            args,
            cwd: std::env::current_dir().ok()?,
        })
    }

    pub(crate) fn transport_args(&self) -> &[String] {
        &self.args
    }

    pub(crate) fn working_dir(&self) -> &std::path::Path {
        &self.cwd
    }

    pub(crate) fn spawn(
        &self,
        script: &str,
        stdin: Stdio,
        timeout: &str,
    ) -> io::Result<std::process::Child> {
        Command::new("timeout")
            .args(["-k", "1", timeout, "ssh"])
            .args(SSH_OPTIONS)
            .args(&self.args)
            .arg(script)
            .current_dir(&self.cwd)
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
    }
}

// Keep routing and credentials from the interactive command. Unknown options
// can change its destination, so decline the background connection if they cannot be replayed.
fn connection_args(argv: &[String]) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut words = argv.get(1..)?.iter().map(String::as_str);
    let mut destination = None;
    while let Some(word) = words.next() {
        match word {
            "--" => {
                destination = destination.or_else(|| words.next());
                break;
            }
            option if option.starts_with('-') => copy_ssh_option(option, &mut words, &mut args)?,
            host if destination.is_none() => destination = Some(host),
            _ => break, // Start of the remote command.
        }
    }
    args.extend(["--".to_owned(), destination?.to_owned()]);
    Some(args)
}

fn copy_ssh_option<'a>(
    option: &'a str,
    words: &mut impl Iterator<Item = &'a str>,
    args: &mut Vec<String>,
) -> Option<()> {
    match option {
        "-t" | "-tt" | "-T" | "-A" | "-a" | "-X" | "-x" | "-Y" | "-C" | "-M" | "-q" | "-v"
        | "-vv" | "-vvv" => return Some(()),
        "-4" | "-6" => args.push(option.to_owned()),
        _ => {
            let (flag, tail) = option.split_at_checked(2)?;
            if !matches!(flag, "-F" | "-i" | "-J" | "-l" | "-p" | "-S" | "-o") {
                return None;
            }
            let value = if tail.is_empty() { words.next()? } else { tail };
            if flag == "-o" && !connection_setting(value) {
                return None;
            }
            args.extend([flag.to_owned(), value.to_owned()]);
        }
    }
    Some(())
}

fn connection_setting(value: &str) -> bool {
    const KEYS: &[&str] = &[
        "BatchMode",
        "HostName",
        "User",
        "Port",
        "IdentityFile",
        "IdentitiesOnly",
        "ProxyJump",
        "ControlPath",
        "ControlMaster",
        "ControlPersist",
        "ConnectTimeout",
        "ServerAliveInterval",
        "ServerAliveCountMax",
        "UserKnownHostsFile",
        "GlobalKnownHostsFile",
        "HostKeyAlias",
        "StrictHostKeyChecking",
    ];
    let key = value.split(['=', ' ']).next().unwrap_or("");
    KEYS.iter().any(|known| known.eq_ignore_ascii_case(key))
}

#[cfg(test)]
pub(crate) struct FixtureClient(std::process::Child);

#[cfg(test)]
impl FixtureClient {
    pub(crate) fn start(config: &std::path::Path) -> Self {
        let child = Command::new("ssh")
            .arg("-F")
            .arg(config)
            .args(["-oBatchMode=yes", "--", "akhterm-fixture", "cat"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        Self(child)
    }

    pub(crate) fn pid(&self) -> i32 {
        self.0.id() as i32
    }

    pub(crate) fn route(&self) -> SshRoute {
        for _ in 0..50 {
            if let Some(route) = SshRoute::from_client(self.pid(), "akhterm-fixture") {
                return route;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("SSH fixture did not become ready");
    }
}

#[cfg(test)]
impl Drop for FixtureClient {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;

    #[test]
    fn ssh_options_require_known_flags_and_complete_values() {
        let args = |words: &[&str]| {
            words
                .iter()
                .map(|word| word.to_string())
                .collect::<Vec<_>>()
        };
        let valid = args(&["ssh", "-4", "-oUsEr=deploy", "prod", "--", "cmd", "-p9"]);
        assert_eq!(
            connection_args(&valid),
            Some(args(&["-4", "-o", "UsEr=deploy", "--", "prod"]))
        );
        for invalid in [
            &["ssh", "-4x", "prod"][..],
            &["ssh", "prod", "-p"],
            &["ssh", "--"],
        ] {
            assert_eq!(connection_args(&args(invalid)), None);
        }
    }

    #[test]
    fn ssh_transport_preserves_routing_and_stops_at_the_remote_command() {
        let args = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            connection_args(&args(
                "ssh -t -p2222 -J bastion -i keys/id box tmux -L other"
            )),
            Some(args("-p 2222 -J bastion -i keys/id -- box"))
        );
        assert_eq!(
            connection_args(&args("ssh box -l deploy -p 22")),
            Some(args("-l deploy -p 22 -- box"))
        );
        assert_eq!(
            connection_args(&args("ssh -t -- ssh://dev@box:2202 tmux")),
            Some(args("-- ssh://dev@box:2202"))
        );
        assert_eq!(
            connection_args(&args("ssh -o ProxyCommand=custom box")),
            None
        );
    }
}
