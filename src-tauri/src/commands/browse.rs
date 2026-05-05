//! Filesystem browse command — backs the path-picker in the settings UI.
//! Mirrors `/api/browse` from the Python app.

use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct BrowseEntry {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowseResult {
    pub cwd: String,
    pub parent: Option<String>,
    pub home: String,
    pub entries: Vec<BrowseEntry>,
}

fn home_dir() -> PathBuf {
    PathBuf::from(shellexpand::tilde("~").into_owned())
}

#[tauri::command]
pub async fn browse_fs(
    path: Option<String>,
    kind: Option<String>,
    show_hidden: Option<bool>,
) -> Result<BrowseResult, String> {
    let kind = kind.unwrap_or_else(|| "any".into());
    let show_hidden = show_hidden.unwrap_or(false);
    let home = home_dir();

    // Expand `~`. Empty / unset → home.
    let mut p = match path.as_deref() {
        Some(s) if !s.is_empty() => PathBuf::from(shellexpand::tilde(s).into_owned()),
        _ => home.clone(),
    };
    // Best-effort canonicalize; fall back to home on failure or non-dir.
    p = std::fs::canonicalize(&p).unwrap_or_else(|_| home.clone());
    if !p.is_dir() {
        p = home.clone();
    }

    let mut entries: Vec<BrowseEntry> = Vec::new();
    if let Ok(read_dir) = std::fs::read_dir(&p) {
        for entry in read_dir.flatten() {
            let name = match entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let is_dir = match entry.file_type() {
                Ok(ft) => ft.is_dir(),
                Err(_) => continue,
            };
            if kind == "dir" && !is_dir {
                continue;
            }
            entries.push(BrowseEntry { name, is_dir });
        }
    }
    // Sort: dirs first, then alphabetical case-insensitive.
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let parent = p.parent().and_then(|par| {
        if par == p.as_path() {
            None
        } else {
            Some(par.to_string_lossy().into_owned())
        }
    });

    Ok(BrowseResult {
        cwd: p.to_string_lossy().into_owned(),
        parent,
        home: home.to_string_lossy().into_owned(),
        entries,
    })
}
