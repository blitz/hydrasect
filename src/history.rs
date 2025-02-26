use log::error;
use std::{env, fs::File, io::ErrorKind, path::PathBuf};

pub fn history_file_path() -> Result<PathBuf, String> {
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

    Ok(path)
}

pub fn open_history_file() -> Result<File, String> {
    let path = history_file_path()?;

    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            error!("Failed to open history file. Please run hydrascrape.");
            return Err("history file not available".to_string());
        }
        Err(e) => {
            return Err(format!("opening history file: {}", e));
        }
    };

    // TODO Consider auto-updating when the history is stale or HEAD
    // is not an ancestor of the last evaluated Git commit.

    Ok(file)
}
