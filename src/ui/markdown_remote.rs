//! Read a document using the endpoint of the SSH process in the clicked pane.
use super::file;
use crate::{paths, proc_radar, telemetry::ssh_connection::SshRoute};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    process::Stdio,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection(SshRoute);

impl Connection {
    pub fn from_client(pid: i32, host: &str) -> Option<Self> {
        SshRoute::from_client(pid, host).map(Self)
    }

    pub fn read(&self, path: &Path) -> io::Result<String> {
        let script = read_command(path)?;
        let mut child = self.0.spawn(&script, Stdio::null(), "15")?;
        let result = file::read_text(child.stdout.take().expect("piped SSH output"));
        let status = child.wait()?;
        if !status.success() {
            return Err(io::Error::other("SSH could not read this file"));
        }
        result
    }

    pub fn paste(&self, address: &TmuxAddress, quote: &str) -> io::Result<()> {
        let mut child = self
            .0
            .spawn(&address.paste_command(), Stdio::piped(), "15")?;
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

#[test]
#[ignore = "set AKHTERM_TEST_SSH_CONFIG to an isolated OpenSSH config for a local test peer named akhterm-fixture"]
fn reads_remote_files_through_the_original_openssh_client_options() {
    use crate::telemetry::ssh_connection::FixtureClient;
    use std::fs;
    let config = std::env::var_os("AKHTERM_TEST_SSH_CONFIG").unwrap();
    let peer = FixtureClient::start(Path::new(&config));
    let c = Connection(peer.route());
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
