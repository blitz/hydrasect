// SPDX-FileCopyrightText: 2022 Alyssa Ross <hi@alyssa.is>
// SPDX-License-Identifier: EUPL-1.2

use std::collections::{BTreeMap, BTreeSet};
use std::env::args;
use std::ffi::OsStr;
use std::fmt::{self, Debug, Display, Formatter};
use std::io::{self, BufRead, BufReader};
use std::iter::once;
use std::os::unix::prelude::*;
use std::process::{exit, Command, ExitStatus, Stdio};
use std::str;

use hydrasect::history::open_history_file;
use log::{debug, info};

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

#[derive(Debug, Default, Eq, PartialEq)]
struct Commit {
    parents: BTreeSet<Oid>,
    children: BTreeSet<Oid>,
}

#[derive(Debug, PartialEq)]
struct CommitGraph {
    bad: Option<Oid>,
    commits: BTreeMap<Oid, Commit>,
}

fn commit_graph(input: impl BufRead) -> Result<CommitGraph, String> {
    fn parse_oid(s: &[u8]) -> Result<Oid, String> {
        Oid::parse(s).map_err(|e| e.to_string())
    }

    fn parse_line(line: io::Result<Vec<u8>>) -> Result<(Oid, BTreeSet<Oid>), String> {
        let line = line.map_err(|e| format!("reading commit graph: {}", e))?;
        let mut fields = line.split(|b| *b == b' ');
        let oid = fields.next().ok_or_else(|| "empty line".to_string())?;
        let parents = fields.map(parse_oid).collect::<Result<_, _>>()?;
        Ok((parse_oid(oid)?, parents))
    }

    let mut parsed_lines = input.split(b'\n').map(parse_line).peekable();

    let bad = match parsed_lines.peek() {
        None => None,
        Some(Err(e)) => return Err(e.clone()),
        Some(Ok((oid, _))) => Some(oid.clone()),
    };

    let dag = parsed_lines.collect::<Result<BTreeMap<_, _>, String>>()?;

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

    Ok(CommitGraph {
        bad,
        commits: undirected_graph,
    })
}

#[test]
fn test_commit_graph() {
    assert_eq!(
        commit_graph(&*b"AA BB CC\nCC DD\n".to_vec()).unwrap(),
        CommitGraph {
            bad: Some(Oid::parse(b"AA").unwrap()),
            commits: vec![
                (
                    Oid::parse(b"AA").unwrap(),
                    Commit {
                        parents: once(b"CC").map(|o| Oid::parse(o).unwrap()).collect(),
                        children: BTreeSet::new(),
                    }
                ),
                (
                    Oid::parse(b"CC").unwrap(),
                    Commit {
                        parents: BTreeSet::new(),
                        children: once(Oid::parse(b"AA").unwrap()).collect(),
                    }
                ),
            ]
            .into_iter()
            .collect()
        }
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

fn bool_status_to_result(status: ExitStatus, name: &'static str) -> Result<bool, String> {
    if status.code() == Some(1) {
        return Ok(false);
    }
    status_to_result(status, name)?;
    Ok(true)
}

fn bisect_graph() -> Result<CommitGraph, String> {
    let mut child = Command::new("git")
        .args(["log", "--format=%H %P", "--bisect"])
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
    graph: CommitGraph,
    mut targets: BTreeSet<Oid>,
    filter: impl Fn(&Oid) -> Result<bool, String>,
) -> Result<BTreeSet<Oid>, String> {
    let mut candidates: BTreeSet<_> = once(start).collect();
    let mut checked = BTreeSet::<Oid>::new();

    if let Some(ref bad) = graph.bad {
        targets.remove(bad);
    }

    loop {
        if candidates.is_empty() {
            return Ok(candidates);
        }

        let matches: BTreeSet<_> = candidates
            .intersection(&targets)
            .map(|oid| filter(oid).map(|r| (oid, r)))
            .filter(|res| !matches!(res, Ok((_, false))))
            .collect::<Result<BTreeSet<_>, _>>()?
            .into_iter()
            .map(|(oid, _)| oid.clone())
            .collect();
        if !matches.is_empty() {
            return Ok(matches);
        }

        let new_candidates = candidates
            .iter()
            .flat_map(|candidate| {
                let commit = graph.commits.get(candidate).unwrap();
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
fn test_closest_commits_skip() {
    let oid = Oid::parse(b"AA").unwrap();
    let graph = CommitGraph {
        bad: None,
        commits: once((oid.clone(), Commit::default())).collect(),
    };
    let history = once(oid.clone()).collect();
    fn pred(_: &Oid) -> Result<bool, String> {
        Ok(false)
    }

    assert!(closest_commits(oid, graph, history, pred)
        .unwrap()
        .is_empty());
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
    fn pred(_: &Oid) -> Result<bool, String> {
        Ok(true)
    }

    let actual = closest_commits(Oid::parse(b"CC").unwrap(), graph, history, pred).unwrap();
    let expected = [b"FF"]
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

fn commit_not_skipped(oid: &Oid) -> Result<bool, String> {
    let status = Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/bisect/skip-{}", oid),
        ])
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("spawning git rev-parse --verify: {}", e))?;

    Ok(!bool_status_to_result(status, "git rev-parse --verify")?)
}

fn run() -> Result<(), String> {
    let history_file = open_history_file()
        .map(BufReader::new)
        .map_err(|e| format!("opening history file: {}", e))?;
    let history = read_history(history_file).map_err(|e| format!("reading history file: {}", e))?;
    let head = git_rev_parse("HEAD").map_err(|e| format!("resolving HEAD: {}", e))?;
    let graph = bisect_graph().map_err(|e| format!("finding bisect graph: {}", e))?;
    let commits = closest_commits(head, graph, history, commit_not_skipped)
        .map_err(|e| format!("finding closest commits: {}", e))?;

    for commit in commits {
        println!("{}", commit);
    }

    Ok(())
}

fn main() {
    simple_logger::SimpleLogger::new()
        .env()
        .without_timestamps()
        .init()
        .unwrap();
    info!("Info message");

    let argv0_option = args().next();
    let argv0 = argv0_option.as_deref().unwrap_or("hydrasect-search");

    if let Err(e) = run() {
        eprintln!("{}: {}", argv0, e);
        exit(1);
    }
}
