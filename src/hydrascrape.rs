use std::{fs::create_dir_all, io::Write};

use anyhow::Result;
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

fn main() -> Result<()> {
    eprintln!("Scraping all {PROJECT}/{JOBSET} evaluations from {HYDRA_URL}...");

    let progress = indicatif::ProgressBar::no_length();
    let client = Client::new();

    let mut page_suffix: String = "".to_string();

    let history_file_path = history_file_path().expect("failed to open history file");
    let mut history_file_dir = history_file_path.clone();
    history_file_dir.pop();

    create_dir_all(&history_file_dir)?;

    let mut history_file = NamedTempFile::new()?;

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

    Ok(())
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
