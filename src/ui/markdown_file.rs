use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const MAX_BYTES: u64 = 2 * 1024 * 1024;

pub fn accepts(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

pub fn read(path: &Path) -> io::Result<String> {
    let file = File::options().read(true).custom_flags(0o4000).open(path)?; // O_NONBLOCK for FIFOs
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Err(io::Error::other(
            "Markdown preview requires a regular file up to 2 MiB",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    decode(bytes)
}

pub fn decode(bytes: Vec<u8>) -> io::Result<String> {
    let source = String::from_utf8(bytes).map_err(io::Error::other)?;
    if source.len() as u64 > MAX_BYTES || source.contains('\0') {
        return Err(io::Error::other("Invalid Markdown text"));
    }
    Ok(source)
}

fn fence(line: &str) -> Option<(char, usize, &str)> {
    let line = line.trim_end().trim_start_matches(' ');
    let ch = line.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let len = line.chars().take_while(|&c| c == ch).count();
    (len >= 3).then(|| (ch, len, &line[len..]))
}

pub fn code_blocks(source: &str) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    let mut open = None;
    let mut byte = 0;
    for line in source.split_inclusive('\n') {
        if let Some((ch, len, suffix)) = fence(line) {
            match open {
                None => open = Some((ch, len, byte + line.len())),
                Some((symbol, min, start)) if ch == symbol && len >= min && suffix.is_empty() => {
                    result.push((start, byte));
                    open = None;
                }
                _ => {}
            }
        }
        byte += line.len();
    }
    if let Some((_, _, start)) = open {
        result.push((start, byte));
    }
    result
}

pub fn byte_at(text: &str, character: i32) -> usize {
    text.char_indices()
        .nth(character.max(0) as usize)
        .map_or(text.len(), |(i, _)| i)
}

pub fn last_path(output: &str, cwd: &str) -> Option<PathBuf> {
    output
        .lines()
        .rev()
        .flat_map(|line| line.split(['`', '"', '\'', '(', ')', '[', ']']).rev())
        .flat_map(|chunk| std::iter::once(chunk.trim()).chain(chunk.split_whitespace().rev()))
        .filter_map(|word| crate::proc_radar::resolve_link_target(word, Some(cwd)))
        .map(PathBuf::from)
        .find(|p| accepts(p) && p.is_file())
}
