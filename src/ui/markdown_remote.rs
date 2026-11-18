//! Read a document using the endpoint of the SSH process in the clicked pane.
use super::file;
use crate::{paths, proc_radar, ssh_detect};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

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
        if !path.is_absolute() {
            return Err(io::Error::other("Expected an absolute path"));
        }
        let name = proc_radar::shell_quote(
            path.to_str()
                .ok_or_else(|| io::Error::other("Invalid path"))?,
        );
        let command = format!("test -f {name} && head -c {} < {name}", file::MAX_BYTES + 1);
        let mut child = self.command(&command).stdin(Stdio::null()).spawn()?;
        let mut out = Vec::new();
        let result = child
            .stdout
            .take()
            .unwrap()
            .take(file::MAX_BYTES + 1)
            .read_to_end(&mut out);
        let status = child.wait()?;
        result?;
        if !status.success() {
            return Err(io::Error::other("SSH could not read this file"));
        }
        file::decode(out)
    }

    fn command(&self, script: &str) -> Command {
        let mut command = Command::new("timeout");
        command
            .args([
                "-k",
                "1",
                "15",
                "ssh",
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
            ])
            .args(&self.args)
            .arg(script)
            .current_dir(&self.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        command
    }

    pub fn paste(&self, address: &TmuxAddress, quote: &str) -> io::Result<()> {
        let mut child = self
            .command(&address.paste_command())
            .stdin(Stdio::piped())
            .spawn()?;
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
        let server = format!("#{{==:#{{pid}},{}}}", self.server);
        let agent = format!("#{{==:#{{pane_current_command}},{}}}", self.cmd);
        let focused = "#{&&:#{pane_active},#{window_active}}";
        let ready = "#{==:#{pane_in_mode},0}";
        let gate = format!("#{{&&:{server},#{{&&:{agent},#{{&&:{focused},{ready}}}}}}}");
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
    let mut it = argv[1..].iter();
    let mut destination = None;
    while let Some(arg) = it.next() {
        if arg == "--" {
            if destination.is_none() {
                destination = it.next();
            }
            break;
        }
        if !arg.starts_with('-') {
            if destination.is_some() {
                break;
            }
            destination = Some(arg);
            continue;
        }
        if [
            "-t", "-tt", "-T", "-A", "-a", "-X", "-x", "-Y", "-q", "-v", "-vv", "-vvv",
        ]
        .contains(&arg.as_str())
        {
            continue;
        }
        let opt = arg.get(..2)?;
        if !["-F", "-i", "-J", "-l", "-p", "-S", "-4", "-6", "-o"].contains(&opt) {
            return None;
        }
        if matches!(opt, "-4" | "-6") {
            if arg != opt {
                return None;
            }
            args.push(arg.clone());
            continue;
        }
        let value = if arg.len() > 2 { &arg[2..] } else { it.next()? };
        if opt == "-o" {
            let key = value.split(['=', ' ']).next()?.to_ascii_lowercase();
            if !matches!(
                key.as_str(),
                "batchmode"
                    | "hostname"
                    | "user"
                    | "port"
                    | "identityfile"
                    | "identitiesonly"
                    | "proxyjump"
                    | "controlpath"
                    | "connecttimeout"
                    | "serveraliveinterval"
                    | "serveralivecountmax"
                    | "userknownhostsfile"
                    | "globalknownhostsfile"
                    | "hostkeyalias"
                    | "stricthostkeychecking"
            ) {
                return None;
            }
        }
        args.extend([opt.to_owned(), value.to_owned()]);
    }
    args.extend(["--".to_owned(), destination?.clone()]);
    Some(args)
}

pub fn candidate(raw: &str) -> Option<PathBuf> {
    let raw = paths::strip_anchor_or_line(paths::strip_link_punctuation(raw));
    let path = if raw.starts_with("file://") {
        crate::session::parse_osc7_directory_uri(raw)?
    } else {
        if raw.contains("://") {
            return None;
        }
        raw.to_owned()
    };
    let path = PathBuf::from(path);
    (file::accepts(&path) && !path.to_string_lossy().chars().any(char::is_control)).then_some(path)
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
    let config = std::env::var("AKHTERM_TEST_SSH_CONFIG").unwrap();
    let mut client = Command::new("ssh")
        .args([
            "-F",
            &config,
            "-oBatchMode=yes",
            "--",
            "akhterm-fixture",
            "cat",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    struct Peer(std::process::Child);
    impl Drop for Peer {
        fn drop(&mut self) {
            let _ = self.0.stdin.take();
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _stdin = client.stdin.take();
    let peer = Peer(client);
    let c = (0..50)
        .find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            Connection::from_client(peer.0.id() as i32, "akhterm-fixture")
        })
        .expect("live SSH process with reproducible connection options");
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
