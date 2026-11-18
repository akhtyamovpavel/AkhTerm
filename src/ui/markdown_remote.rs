//! Read a document using the endpoint of the SSH process in the clicked pane.
use super::file;
use crate::{paths, proc_radar, ssh_detect};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    args: Vec<String>,
    cwd: PathBuf,
}

impl Connection {
    pub fn from_client(pid: i32, host: &str) -> Option<Self> {
        let argv = proc_radar::read_proc_cmdline(pid)?;
        if ssh_detect::parse_ssh_argv(&argv)?.as_str() != host {
            return None;
        }
        Some(Self {
            args: connection_args(&argv)?,
            cwd: std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?,
        })
    }

    pub fn read(&self, path: &Path) -> io::Result<String> {
        let script = read_command(path)?;
        let mut child = self.spawn(&script, Stdio::null())?;
        let result = file::read_text(child.stdout.take().expect("piped SSH output"));
        let status = child.wait()?;
        if !status.success() {
            return Err(io::Error::other("SSH could not read this file"));
        }
        result
    }

    fn spawn(&self, script: &str, stdin: Stdio) -> io::Result<std::process::Child> {
        Command::new("timeout")
            .args(["-k", "1", "15", "ssh"])
            .args(SSH_OPTIONS)
            .args(&self.args)
            .arg(script)
            .current_dir(&self.cwd)
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
    }

    pub fn paste(&self, address: &TmuxAddress, quote: &str) -> io::Result<()> {
        let mut child = self.spawn(&address.paste_command(), Stdio::piped())?;
        let written = child.stdin.take().unwrap().write_all(quote.as_bytes());
        let output = child.wait_with_output()?;
        written?;
        if output.status.success() && output.stdout == b"inserted\n" {
            return Ok(());
        }
        Err(io::Error::other(
            "SSH or tmux could not insert the quote into the selected agent",
        ))
    }
}

fn read_command(path: &Path) -> io::Result<String> {
    if !path.is_absolute() {
        return Err(io::Error::other("Expected an absolute path"));
    }
    let name = proc_radar::shell_quote(
        path.to_str()
            .ok_or_else(|| io::Error::other("Invalid path"))?,
    );
    Ok(format!(
        "test -f {name} && head -c {} < {name}",
        file::MAX_BYTES + 1
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TmuxAddress {
    server: u32,
    pane: u32,
    cmd: String,
}

impl TmuxAddress {
    pub fn new(server: u32, pane: &str, cmd: &str) -> Option<Self> {
        let pane = pane.strip_prefix('%')?.parse().ok()?;
        if cmd.is_empty()
            || !cmd
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return None;
        }
        Some(Self {
            server,
            pane,
            cmd: cmd.to_owned(),
        })
    }

    pub(super) fn paste_command(&self) -> String {
        // The quote travels over stdin. tmux checks the server, pane and agent
        // on its command queue before pasting into this specific pane.
        let state =
            "#{pid}:#{pane_current_command}:#{pane_active}:#{window_active}:#{pane_in_mode}";
        let expected = format!("{}:{}:1:1:0", self.server, self.cmd);
        let gate = format!("#{{==:{state},{expected}}}");
        format!(
            r#"b=akhterm-quote-$$
trap 'tmux delete-buffer -b "$b" 2>/dev/null || :' EXIT
tmux load-buffer -b "$b" - && tmux if-shell -F -t %{p} '{gate}' "paste-buffer -p -r -d -b $b -t %{p} ; display-message -p inserted" "display-message -p stale""#,
            p = self.pane
        )
    }
}

// Keep routing and credentials from the interactive command. Unknown options
// can change its destination, so decline the preview if they cannot be replayed.
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
        "-t" | "-tt" | "-T" | "-A" | "-a" | "-X" | "-x" | "-Y" | "-q" | "-v" | "-vv" | "-vvv" => {
            return Some(())
        }
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

pub fn candidate(raw: &str) -> Option<PathBuf> {
    let raw = paths::strip_anchor_or_line(paths::strip_link_punctuation(raw));
    match raw.split_once("://") {
        Some(("file", _)) => preview_path(&crate::session::parse_osc7_directory_uri(raw)?),
        Some(_) => None,
        None => preview_path(raw),
    }
}

pub fn preview_path(value: &str) -> Option<PathBuf> {
    let path = Path::new(value);
    (file::accepts(path) && !value.chars().any(char::is_control)).then(|| path.to_owned())
}

pub fn resolve(path: &Path, cwd: Option<&Path>) -> Option<PathBuf> {
    if path.is_absolute() {
        return Some(path.to_owned());
    }
    if path.to_string_lossy().starts_with('~') {
        return None;
    }
    Some(cwd.filter(|p| p.is_absolute())?.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) struct FixtureClient(std::process::Child);

    impl FixtureClient {
        pub(super) fn start() -> Self {
            let config = std::env::var("AKHTERM_TEST_SSH_CONFIG").unwrap();
            let mut ssh = Command::new("ssh");
            ssh.args([
                "-F",
                &config,
                "-oBatchMode=yes",
                "--",
                "akhterm-fixture",
                "cat",
            ]);
            Self(
                ssh.stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .spawn()
                    .unwrap(),
            )
        }

        pub(super) fn connection(&self) -> Connection {
            let pid = self.0.id() as i32;
            for _ in 0..50 {
                if let Some(connection) = Connection::from_client(pid, "akhterm-fixture") {
                    return connection;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("SSH fixture did not become ready");
        }
    }

    impl Drop for FixtureClient {
        fn drop(&mut self) {
            drop(self.0.stdin.take());
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

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

#[cfg(test)]
#[test]
#[ignore = "set AKHTERM_TEST_SSH_CONFIG to an isolated OpenSSH config for a local test peer named akhterm-fixture"]
fn reads_remote_files_through_the_original_openssh_client_options() {
    use std::fs;
    let peer = tests::FixtureClient::start();
    let c = peer.connection();
    let dir = std::env::temp_dir().join(format!("akhterm-ssh-test-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let path = dir.join("план '$x'.md");
    let outcome = (|| -> io::Result<()> {
        fs::write(&path, "раздел один")?;
        assert_eq!(c.read(&path)?, "раздел один");
        fs::write(&path, "новая редакция")?;
        assert_eq!(c.read(&path)?, "новая редакция");
        fs::write(&path, vec![b'x'; file::MAX_BYTES as usize + 1])?;
        assert!(c.read(&path).is_err());
        assert!(c.read(&dir).is_err());
        Ok(())
    })();
    fs::remove_dir_all(dir).unwrap();
    outcome.unwrap();
}
