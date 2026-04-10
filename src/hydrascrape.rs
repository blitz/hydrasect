use std::{
    env,
    fs::{create_dir_all, File},
    io::{BufRead, BufReader, Write},
    process::ExitCode,
};

use anyhow::{bail, Result};
use reqwest::{
    blocking::Client,
    header::{HeaderMap, ACCEPT, USER_AGENT},
};
use serde_json::Value;
use tempfile::NamedTempFile;

use hydrasect::history::history_file_path;

const HYDRA_URL: &str = "https://hydra.nixos.org";
const PROJECT: &str = "nixos";
const JOBSET: &str = "unstable-small";

fn fetch_page(client: &Client, page_suffix: &str) -> Result<Value> {
    let url = format!("{HYDRA_URL}/jobset/{PROJECT}/{JOBSET}/evals{}", page_suffix);

    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, "application/json".parse().unwrap());
    headers.insert(USER_AGENT, "hydrasect".parse().unwrap());

    Ok(client.get(url).headers(headers).send()?.json()?)
}

fn parse_page(page_suffix: &str) -> Option<u32> {
    page_suffix
        .split_once("=")
        .and_then(|(_first, second)| second.parse().ok())
}

fn parse_args() -> Result<Option<u64>> {
    let mut from: Option<u64> = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--from=") {
            from = Some(value.parse()?);
        } else if arg == "--from" {
            let value = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("--from requires a value"))?;
            from = Some(value.parse()?);
        } else {
            bail!("unknown argument: {arg}");
        }
    }
    Ok(from)
}

fn main() -> Result<ExitCode> {
    let from = match parse_args() {
        Ok(from) => from,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!(
                "\nUsage: hydrascrape [--from <eval_id>]\n\n\
                 --from <eval_id>    Stop scraping once all remaining evaluations\n\
                                     have an id lower than <eval_id>. Useful to\n\
                                     avoid re-fetching Hydra's entire history."
            );
            return Ok(ExitCode::from(2));
        }
    };

    if let Some(from) = from {
        eprintln!(
            "Scraping {PROJECT}/{JOBSET} evaluations from {HYDRA_URL} (from eval id {from})..."
        );
    } else {
        eprintln!("Scraping all {PROJECT}/{JOBSET} evaluations from {HYDRA_URL}...");
    }

    let progress = indicatif::ProgressBar::no_length();
    let client = Client::new();

    let mut page_suffix: String = "".to_string();

    let history_file_path = history_file_path().expect("failed to open history file");
    let mut history_file_dir = history_file_path.clone();
    history_file_dir.pop();

    create_dir_all(&history_file_dir)?;

    let mut history_file = NamedTempFile::new_in(&history_file_dir)?;
    let mut reached_from = false;
    let mut max_eval_id: Option<u64> = None;

    // When --from is given, preserve older entries from the existing
    // history file so we do not lose data we are deliberately not
    // re-scraping.
    if let Some(from) = from {
        match File::open(&history_file_path) {
            Ok(existing) => {
                let mut preserved = 0usize;
                for line in BufReader::new(existing).lines() {
                    let line = line?;
                    let eval_id: u64 = match line.split_whitespace().nth(1) {
                        Some(s) => match s.parse() {
                            Ok(v) => v,
                            Err(_) => continue,
                        },
                        None => continue,
                    };
                    if eval_id < from {
                        history_file.write_all(line.as_bytes())?;
                        history_file.write_all(b"\n")?;
                        preserved += 1;
                    }
                }
                eprintln!("Preserved {preserved} existing entries below eval id {from}.");
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }

    loop {
        progress.set_position(parse_page(&page_suffix).unwrap_or(1).into());

        let page_content = fetch_page(&client, &page_suffix)?;
        let current_page = page_content.as_object().expect("expected object");

        if progress.length().is_none() {
            let last_page_str = current_page
                .get("last")
                .expect("expected key last")
                .as_str()
                .expect("expected string");
            if let Some(last_page) = parse_page(last_page_str) {
                progress.set_length(last_page.into());
            }
        }

        for eval in current_page
            .get("evals")
            .expect("expected evals key")
            .as_array()
            .expect("expected array")
        {
            let eval = eval.as_object().expect("expected object");
            let eval_id = eval
                .get("id")
                .expect("expected key id")
                .as_u64()
                .expect("expected integer");

            if let Some(from) = from {
                if eval_id < from {
                    reached_from = true;
                    continue;
                }
            }

            if max_eval_id.map_or(true, |m| eval_id > m) {
                max_eval_id = Some(eval_id);
            }

            let inputs = eval
                .get("jobsetevalinputs")
                .expect("expected key jobsetevalinputs")
                .as_object()
                .expect("expected object");

            let nixpkgs = inputs.get("nixpkgs").expect("expected key nixpkgs");
            let revision = nixpkgs
                .get("revision")
                .expect("expected key revision")
                .as_str()
                .expect("expected string")
                .to_owned();

            history_file.write_all(format!("{revision} {eval_id}\n").as_bytes())?;
        }

        if reached_from {
            break;
        }

        if let Some(next_page_suffix) = current_page.get("next") {
            page_suffix = next_page_suffix
                .as_str()
                .expect("expected string")
                .to_owned();
        } else {
            break;
        }
    }

    eprintln!("Replacing old history file with new data.");
    history_file.into_temp_path().persist(history_file_path)?;

    if let Some(max) = max_eval_id {
        eprintln!(
            "Newest eval id fetched: {max}. Pass `--from {max}` on the next run \
             to only fetch newer evaluations."
        );
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_parse_page_suffix() {
        assert_eq!(parse_page(""), None);
        assert_eq!(parse_page("xxx"), None);
        assert_eq!(parse_page("?page=588"), Some(588));
        assert_eq!(parse_page("?page=xxx"), None);
    }
}
