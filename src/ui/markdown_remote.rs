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

fn check_paste_output(output: &std::process::Output) -> io::Result<()> {
    if output.status.success() && output.stdout == b"inserted\n" {
        return Ok(());
    }
    Err(io::Error::other(
        "SSH or tmux could not insert the quote into the selected agent",
    ))
}

fn write_child_stdin(child: &mut std::process::Child, payload: &str) -> io::Result<()> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("missing SSH stdin"))?;
    stdin.write_all(payload.as_bytes())
}

impl Connection {
    pub fn from_client(pid: i32, host: &str) -> Option<Self> {
        SshRoute::from_client(pid, host).map(Self)
    }

    pub fn read(&self, path: &Path) -> io::Result<String> {
        let script = read_command(path)?;
        let mut child = self.0.spawn(&script, Stdio::null(), "15")?;
        let result = file::read_text(child.stdout.take().expect("piped SSH output"));
        if !child.wait()?.success() {
            return Err(io::Error::other("SSH could not read this file"));
        }
        result
    }

    pub fn paste(&self, address: &TmuxAddress, quote: &str) -> io::Result<()> {
        let mut child = self
            .0
            .spawn(&address.paste_command(), Stdio::piped(), "15")?;
        let written = write_child_stdin(&mut child, quote);
        let output = child.wait_with_output()?;
        written?;
        check_paste_output(&output)
    }
}

fn quote_absolute_path(path: &Path) -> io::Result<String> {
    if !path.is_absolute() {
        return Err(io::Error::other("Expected an absolute path"));
    }
    let raw = path
        .to_str()
        .ok_or_else(|| io::Error::other("Invalid path"))?;
    Ok(proc_radar::shell_quote(raw))
}

fn read_command(path: &Path) -> io::Result<String> {
    let name = quote_absolute_path(path)?;
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

fn is_valid_agent_cmd(cmd: &str) -> bool {
    !cmd.is_empty()
        && cmd
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

impl TmuxAddress {
    pub fn new(server: u32, pane: &str, cmd: &str) -> Option<Self> {
        let pane = pane.strip_prefix('%')?.parse().ok()?;
        if !is_valid_agent_cmd(cmd) {
            return None;
        }
        Some(Self {
            server,
            pane,
            cmd: cmd.to_owned(),
        })
    }

    fn tmux_gate_expr(&self) -> String {
        let state =
            "#{pid}:#{pane_current_command}:#{pane_active}:#{window_active}:#{pane_in_mode}";
        let expected = format!("{}:{}:1:1:0", self.server, self.cmd);
        format!("#{{==:{state},{expected}}}")
    }

    pub fn paste_command(&self) -> String {
        // The quote travels over stdin. tmux checks the server, pane and agent
        // on its command queue before pasting into this specific pane.
        let gate = self.tmux_gate_expr();
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

#[cfg(test)]
fn verify_remote_fixture_reads(conn: &Connection, dir: &Path) -> io::Result<()> {
    let path = dir.join("план '$x'.md");
    std::fs::write(&path, "раздел один")?;
    assert_eq!(conn.read(&path)?, "раздел один");
    std::fs::write(&path, "новая редакция")?;
    assert_eq!(conn.read(&path)?, "новая редакция");
    std::fs::write(&path, vec![b'x'; file::MAX_BYTES as usize + 1])?;
    assert!(conn.read(&path).is_err());
    assert!(conn.read(dir).is_err());
    Ok(())
}

#[test]
#[ignore = "set AKHTERM_TEST_SSH_CONFIG to an isolated OpenSSH config for a local test peer named akhterm-fixture"]
fn reads_remote_files_through_the_original_openssh_client_options() {
    use crate::telemetry::ssh_connection::FixtureClient;
    let config = std::env::var_os("AKHTERM_TEST_SSH_CONFIG").unwrap();
    let peer = FixtureClient::start(Path::new(&config));
    let conn = Connection(peer.route());
    let dir = std::env::temp_dir().join(format!("akhterm-ssh-test-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let outcome = verify_remote_fixture_reads(&conn, &dir);
    std::fs::remove_dir_all(dir).unwrap();
    outcome.unwrap();
}
