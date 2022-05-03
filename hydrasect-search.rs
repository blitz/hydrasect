// SPDX-FileCopyrightText: 2022 Alyssa Ross <hi@alyssa.is>
// SPDX-License-Identifier: EUPL-1.2

use std::cmp::min;
use std::collections::{BTreeMap, BTreeSet};
use std::env::{self, args};
use std::ffi::OsStr;
use std::fmt::{self, Debug, Display, Formatter};
use std::fs::{create_dir_all, rename, File};
use std::io::{self, BufRead, BufReader, ErrorKind, Read, Seek, SeekFrom};
use std::os::unix::prelude::*;
use std::path::{Path, PathBuf};
use std::process::{exit, Command, ExitStatus, Stdio};
use std::str;
use std::time::{Duration, SystemTime};

struct OidParseError([u8; 2]);

impl Display for OidParseError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let s = String::from_utf8_lossy(&self.0);
        write!(f, "{:?} cannot be parsed as an octet", s)
    }
}

#[test]
fn test_oid_parse_error_to_string() {
    let actual = OidParseError([b'g', b'h']).to_string();
    assert_eq!(actual, r#""gh" cannot be parsed as an octet"#);
}

impl Debug for OidParseError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "OidParseError({:?})", String::from_utf8_lossy(&self.0))
    }
}

#[test]
fn test_oid_parse_error_debug() {
    let actual = format!("{:?}", OidParseError([b'g', b'h']));
    assert_eq!(actual, r#"OidParseError("gh")"#);
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
struct Oid(Vec<u8>);

impl Oid {
    fn parse(bytes: &[u8]) -> Result<Self, OidParseError> {
        let inner = bytes
            .chunks(2)
            .map(|pair| {
                str::from_utf8(pair)
                    .ok()
                    .and_then(|s| u8::from_str_radix(s, 16).ok())
                    .ok_or(OidParseError([pair[0], pair[1]]))
            })
            .collect::<Result<_, _>>()?;

        Ok(Self(inner))
    }
}

impl Display for Oid {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{:02x}", byte)?;
        }
        Ok(())
    }
}

#[test]
fn test_oid_display() {
    let oid = Oid::parse(b"0011f9065a1ad1da4db67bec8d535d91b0a78fba").unwrap();
    assert_eq!(oid.to_string(), "0011f9065a1ad1da4db67bec8d535d91b0a78fba");
}

impl Debug for Oid {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "Oid({})", self)
    }
}

#[test]
fn test_oid_debug() {
    let oid = Oid::parse(b"0011f9065a1ad1da4db67bec8d535d91b0a78fba").unwrap();
    let debug = format!("{:?}", oid);
    assert_eq!(debug, "Oid(0011f9065a1ad1da4db67bec8d535d91b0a78fba)");
}

#[derive(Debug, Eq, PartialEq)]
struct Commit {
    parents: BTreeSet<Oid>,
    children: BTreeSet<Oid>,
}

fn commit_graph(input: impl BufRead) -> Result<BTreeMap<Oid, Commit>, String> {
    fn parse_oid(s: &[u8]) -> Result<Oid, String> {
        Oid::parse(s).map_err(|e| e.to_string())
    }

    let dag = input
        .split(b'\n')
        .map(|line| {
            let line = line.map_err(|e| format!("reading commit graph: {}", e))?;
            let mut fields = line.split(|b| *b == b' ');
            let oid = fields.next().ok_or_else(|| "empty line".to_string())?;
            let parents = fields.map(parse_oid).collect::<Result<_, _>>()?;
            Ok((parse_oid(oid)?, parents))
        })
        .collect::<Result<BTreeMap<_, BTreeSet<_>>, String>>()?;

    // Create a mapping from parent commits to their children.
    let mut paternities = BTreeMap::<_, BTreeSet<_>>::new();
    for (oid, parents) in &dag {
        for parent in parents {
            paternities
                .entry(parent.clone())
                .or_default()
                .insert(oid.clone());
        }
    }

    let considered_oids: BTreeSet<_> = dag.keys().map(Clone::clone).collect();

    let undirected_graph = dag
        .into_iter()
        .map(|(oid, parents)| {
            let commit = Commit {
                parents: parents
                    .intersection(&considered_oids)
                    .map(Clone::clone)
                    .collect(),
                children: paternities.remove(&oid).unwrap_or_default(),
            };
            (oid, commit)
        })
        .collect();

    Ok(undirected_graph)
}

#[test]
fn test_commit_graph() {
    assert_eq!(
        commit_graph(&*b"AA BB CC\nCC DD\n".to_vec()).unwrap(),
        vec![
            (
                Oid::parse(b"AA").unwrap(),
                Commit {
                    parents: [b"CC"]
                        .into_iter()
                        .map(|o| Oid::parse(o).unwrap())
                        .collect(),
                    children: BTreeSet::new(),
                }
            ),
            (
                Oid::parse(b"CC").unwrap(),
                Commit {
                    parents: BTreeSet::new(),
                    children: [Oid::parse(b"AA").unwrap()].into_iter().collect(),
                }
            ),
        ]
        .into_iter()
        .collect()
    );
}

fn status_to_result(status: ExitStatus, name: &'static str) -> Result<(), String> {
    if let Some(signal) = status.signal() {
        return Err(format!("{} killed by signal {}", name, signal));
    }
    if !status.success() {
        return Err(format!("{} exited {}", name, status.code().unwrap()));
    }
    Ok(())
}

fn bisect_graph() -> Result<BTreeMap<Oid, Commit>, String> {
    let mut child = Command::new("git")
        .args(&["log", "--format=%H %P", "--bisect"])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn git log: {}", e))?;

    let graph_result = commit_graph(BufReader::new(child.stdout.take().unwrap()));

    let status = child
        .wait()
        .map_err(|e| format!("waiting for git: {}", e))?;
    status_to_result(status, "git log")?;

    graph_result.map_err(|e| format!("parsing git log output: {}", e))
}

fn parse_history_line(line: Vec<u8>) -> Oid {
    let oid_str = line
        .into_iter()
        .take_while(u8::is_ascii_hexdigit)
        .collect::<Vec<_>>();
    Oid::parse(&oid_str).unwrap()
}

fn read_history(input: impl BufRead) -> io::Result<BTreeSet<Oid>> {
    input
        .split(b'\n')
        .map(|line| Ok(parse_history_line(line?)))
        .collect()
}

#[test]
fn test_read_history() {
    let input = b"0011f9065a1ad1da4db67bec8d535d91b0a78fba 1496527122\n\
                  0d4431cfe90b2242723ccb1ccc90714f2f68a609 1497692199\n";
    let expected = [
        b"0011f9065a1ad1da4db67bec8d535d91b0a78fba",
        b"0d4431cfe90b2242723ccb1ccc90714f2f68a609",
    ]
    .into_iter()
    .map(|o| Oid::parse(o).unwrap())
    .collect();
    assert_eq!(read_history(&*input.to_vec()).unwrap(), expected);
}

fn closest_commits(
    start: Oid,
    graph: BTreeMap<Oid, Commit>,
    targets: BTreeSet<Oid>,
) -> BTreeSet<Oid> {
    let mut candidates: BTreeSet<_> = [start].into_iter().collect();
    let mut checked = BTreeSet::<Oid>::new();

    loop {
        if candidates.is_empty() {
            return candidates;
        }

        let matches: BTreeSet<_> = candidates
            .intersection(&targets)
            .map(Clone::clone)
            .collect();
        if !matches.is_empty() {
            return matches;
        }

        let new_candidates = candidates
            .iter()
            .flat_map(|candidate| {
                let commit = graph.get(&candidate).unwrap();
                commit.children.union(&commit.parents)
            })
            .map(Clone::clone)
            .collect::<BTreeSet<_>>()
            .difference(&checked)
            .map(Clone::clone)
            .collect();
        checked.append(&mut candidates);
        candidates = new_candidates;
    }
}

#[test]
fn test_closest_commits() {
    let graph = b"AA BB\n\
                  BB CC\n\
                  CC DD EE\n\
                  EE FF\n\
                  FF 00";
    let history = read_history(&*b"AA 0\nFF 0\n00 0\n".to_vec()).unwrap();
    let graph = commit_graph(&*graph.to_vec()).unwrap();

    let actual = closest_commits(Oid::parse(b"CC").unwrap(), graph, history);
    let expected = [b"AA", b"FF"]
        .into_iter()
        .map(|o| Oid::parse(o).unwrap())
        .collect();

    assert_eq!(actual, expected);
}

fn git_rev_parse(commit: impl AsRef<OsStr>) -> Result<Oid, String> {
    let out = Command::new("git")
        .arg("rev-parse")
        .arg(commit)
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("spawning git: {}", e))?;
    status_to_result(out.status, "git rev-parse")?;
    let mut stdout = out.stdout;
    stdout.pop();
    Oid::parse(&stdout).map_err(|e| format!("parsing git rev-parse output: {}", e))
}

fn last_line(reader: &mut (impl Read + Seek)) -> io::Result<Vec<u8>> {
    let mut buf = vec![0; 4096];
    // Skip an extra character the first time to avoid considering a
    // trailing newline.
    let mut from_end = buf.len() as i64 + 1;

    loop {
        match reader.seek(SeekFrom::End(-from_end)) {
            // EINVAL means we tried to seek to before the beginning.
            Err(e) if e.kind() == ErrorKind::InvalidInput => {
                // Avoid trying to read past the end, for the case
                // where the file is smaller than buf.
                let file_len = reader.seek(SeekFrom::End(0))? as usize;
                buf.resize(min(file_len, buf.len()), 0);

                // Clamp our position to the start of the file.
                reader.rewind()?;
            }
            r => {
                r?;
            }
        }

        reader.read_exact(&mut buf)?;

        // Rewind to one character after the last newline we found, if any.
        if let Some(i) = buf.iter().rposition(|b| b == &b'\n') {
            reader.seek(SeekFrom::Current(-(buf.len() as i64) + i as i64 + 1))?;
            break;
        }

        // If we're at the start of the stream, this is the only line
        // in the file, and we're done.
        if reader.stream_position()? as usize - buf.len() == 0 {
            reader.rewind()?;
            break;
        }

        from_end += buf.len() as i64;
    }

    buf.resize(0, 0);
    reader.read_to_end(&mut buf)?;

    if buf.last() == Some(&b'\n') {
        buf.pop();
    }

    Ok(buf)
}

#[cfg(test)]
fn tmpfile() -> io::Result<File> {
    use std::os::raw::c_int;

    extern "C" {
        fn tmpfd() -> c_int;
    }

    let fd = unsafe { tmpfd() };
    if fd == -1 {
        return Err(io::Error::last_os_error());
    }
    unsafe { Ok(File::from_raw_fd(fd)) }
}

#[test]
fn test_last_line_empty() {
    let mut file = tmpfile().unwrap();
    let line = last_line(&mut file).unwrap();
    assert!(line.is_empty());
}

#[test]
fn test_last_line_first() {
    use std::io::{Seek, Write};

    let len = 4096 * 3;
    let mut data = vec![b'a'; len];
    *data.last_mut().unwrap() = b'\n';

    let mut file = tmpfile().unwrap();
    file.write_all(&data).unwrap();
    file.rewind().unwrap();

    let line = last_line(&mut file).unwrap();
    assert_eq!(data[..(len - 1)], line);
}

#[test]
fn test_last_line_short() {
    use std::io::{Seek, Write};

    let len = 1024;
    let mut data = vec![b'a'; len];
    data[len - 10] = b'\n';
    data[len - 9] = b'b';

    let mut file = tmpfile().unwrap();
    file.write_all(&data).unwrap();
    file.rewind().unwrap();

    let line = last_line(&mut file).unwrap();
    assert_eq!(data[(len - 9)..], line);
}

#[test]
fn test_last_line_long() {
    use std::io::{Seek, Write};

    let len = 4096 * 3;
    let mut data = vec![b'a'; len];
    *data.last_mut().unwrap() = b'\n';
    data[len / 2] = b'\n';
    data[len / 2 + 1] = b'b';

    let mut file = tmpfile().unwrap();
    file.write_all(&data).unwrap();
    file.rewind().unwrap();

    let line = last_line(&mut file).unwrap();
    assert_eq!(data[(len / 2 + 1)..(len - 1)], line);
}

fn git_is_ancestor(lhs: &dyn AsRef<OsStr>, rhs: &dyn AsRef<OsStr>) -> Result<bool, String> {
    let status = Command::new("git")
        .args(&["merge-base", "--is-ancestor"])
        .arg(lhs)
        .arg(rhs)
        .status()
        .map_err(|e| format!("spawning git merge-base --is-ancestor: {}", e))?;

    if status.code() == Some(1) {
        return Ok(false);
    }
    status_to_result(status, "git merge-base --is-ancestor")?;
    Ok(true)
}

fn update_history_file(path: &Path) -> Result<File, String> {
    if let Some(parent) = path.parent() {
        let _ = create_dir_all(parent);
    }

    let tmp_path = path.with_extension("tmp");

    let status = Command::new("curl")
        .arg("-fLsSo")
        .arg(&tmp_path)
        .arg("-z")
        .arg(path)
        .arg("https://channels.nix.gsc.io/nixpkgs-unstable/history")
        .status()
        .map_err(|e| format!("spawning curl: {}", e))?;

    if let Some(code) = status.code() {
        if code > 4 && code != 48 {
            eprintln!("Warning: failed to update the Hydra evaluation history file.");
        }
    }
    status_to_result(status, "curl")?;

    match rename(&tmp_path, path) {
        // If the source file doesn't exist, we got a 304 Not Modified,
        // so the existing file is up to date.
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        r => r.map_err(|e| format!("moving new history file into place: {}", e)),
    }?;

    File::open(&path).map_err(|e| format!("opening updated history file: {}", e))
}

fn open_history_file() -> Result<File, String> {
    let mut path: PathBuf = match env::var_os("XDG_CACHE_HOME") {
        Some(v) if !v.is_empty() => v.into(),
        _ => match env::var_os("HOME") {
            Some(v) if !v.is_empty() => {
                let mut path_buf = PathBuf::from(v);
                path_buf.push(".cache");
                path_buf
            }
            _ => {
                return Err("XDG_CACHE_HOME and HOME are both unset or empty".to_string());
            }
        },
    };
    path.push("hydrasect/hydra-eval-history");

    let mut file = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return update_history_file(&path).map_err(|e| format!("updating history file: {}", e))
        }
        Err(e) => {
            return Err(format!("opening history file: {}", e));
        }
    };

    let most_recent_eval = last_line(&mut file)
        .map(parse_history_line)
        .map_err(|e| format!("reading last line of history file: {}", e))?;
    file.rewind().unwrap();

    if !git_is_ancestor(&"refs/bisect/bad", &most_recent_eval.to_string())
        .map_err(|e| format!("checking history freshness: {}", e))?
    {
        let mtime = file
            .metadata()
            .map_err(|e| format!("checking history file metadata: {}", e))?
            .modified()
            .map_err(|e| format!("checking history file modified date: {}", e))?;
        if SystemTime::now()
            .duration_since(mtime)
            .map_err(|e| format!("checking time since history file modification: {}", e))?
            > Duration::from_secs(15 * 60)
        {
            file = update_history_file(&path)?;
        }
    }

    Ok(file)
}

fn run() -> Result<(), String> {
    let history_file = open_history_file()
        .map(BufReader::new)
        .map_err(|e| format!("opening history file: {}", e))?;
    let history = read_history(history_file).map_err(|e| format!("reading history file: {}", e))?;
    let head = git_rev_parse("HEAD").map_err(|e| format!("resolving HEAD: {}", e))?;
    let graph = bisect_graph().map_err(|e| format!("finding bisect graph: {}", e))?;

    for commit in closest_commits(head, graph, history) {
        println!("{}", commit);
    }

    Ok(())
}

fn main() {
    let argv0_option = args().next();
    let argv0 = argv0_option
        .as_ref()
        .map(String::as_str)
        .unwrap_or("hydrasect-search");
    if let Err(e) = run() {
        eprintln!("{}: {}", argv0, e);
        exit(1);
    }
}
