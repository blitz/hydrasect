use std::{fs::create_dir_all, io::Write};

use anyhow::Result;
use reqwest::{
    blocking::Client,
    header::{HeaderMap, ACCEPT},
};
use serde_json::Value;
use tempfile::NamedTempFile;

use hydrasect::history::history_file_path;

const HYDRA_URL: &'static str = "https://hydra.nixos.org";
const PROJECT: &'static str = "nixos";
const JOBSET: &'static str = "unstable-small";

fn fetch_page(client: &Client, page_suffix: &str) -> Result<Value> {
    let url = format!("{HYDRA_URL}/jobset/{PROJECT}/{JOBSET}/evals{}", page_suffix);
    eprintln!("Fetching: {url}");

    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, "application/json".parse().unwrap());

    Ok(client.get(url).headers(headers).send()?.json()?)
}

fn main() -> Result<()> {
    eprintln!("Scraping all {PROJECT}/{JOBSET} evaluations from {HYDRA_URL}...");

    let client = Client::new();

    let mut page_suffix: String = "".to_string();

    let history_file_path = history_file_path().expect("failed to open history file");
    let mut history_file_dir = history_file_path.clone();
    history_file_dir.pop();

    create_dir_all(&history_file_dir)?;

    let mut history_file = NamedTempFile::new()?;

    loop {
        let page_content = fetch_page(&client, &page_suffix)?;
        let current_page = page_content.as_object().expect("expected object");

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
