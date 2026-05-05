//! Profile commands — list/save/load/delete YAML profile files in
//! `<cwd>/profiles/`. Mirrors the `/api/profiles` routes in `app/main.py`.

use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::Regex;

use crate::config::{save_config, AppConfig};
use crate::state::{self, AppState};

static SAFE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^\w\-.]").unwrap());

/// Sanitize a profile name with the same rule as Python's
/// `_safe_profile_name`: replace anything outside `[A-Za-z0-9_\-.]` with `_`,
/// then strip leading/trailing `._-`. Empty result is an error.
pub fn safe_profile_name(name: &str) -> Result<String, String> {
    let cleaned = SAFE_RE.replace_all(name, "_");
    let trimmed = cleaned.trim_matches(|c: char| c == '.' || c == '_' || c == '-');
    if trimmed.is_empty() {
        Err("invalid profile name".into())
    } else {
        Ok(trimmed.to_string())
    }
}

fn profiles_dir() -> PathBuf {
    std::env::var("DDODOLI_PROFILES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("profiles"))
}

fn profile_path(name: &str) -> Result<PathBuf, String> {
    let safe = safe_profile_name(name)?;
    Ok(profiles_dir().join(format!("{safe}.yaml")))
}

#[tauri::command]
pub async fn list_profiles() -> Result<Vec<String>, String> {
    let dir = profiles_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut names = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => return Err(e.to_string()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("yaml") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

#[tauri::command]
pub async fn save_profile(name: String, config: AppConfig) -> Result<String, String> {
    let dir = profiles_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = profile_path(&name)?;
    save_config(&path, &config).map_err(|e| e.to_string())?;
    Ok(path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string())
}

#[tauri::command]
pub async fn load_profile(
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<bool, String> {
    let path = profile_path(&name)?;
    if !Path::new(&path).exists() {
        return Err("profile not found".into());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut new_cfg = AppConfig::from_yaml(&text).map_err(|e| e.to_string())?;
    new_cfg.expand_paths();

    let mut inner = state.0.lock().await;
    // Mirror behavior of FastAPI route: also write into the active config path
    // so the next launch picks up the loaded profile.
    save_config(&inner.cfg_path, &new_cfg).map_err(|e| e.to_string())?;
    state::reload(&mut inner, new_cfg)
        .await
        .map_err(|e| e.to_string())?;
    Ok(inner.running)
}

#[tauri::command]
pub async fn delete_profile(name: String) -> Result<(), String> {
    let path = profile_path(&name)?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_profile_name_basic() {
        assert_eq!(safe_profile_name("hello").unwrap(), "hello");
        assert_eq!(safe_profile_name("a/b\\c").unwrap(), "a_b_c");
        assert_eq!(safe_profile_name("..foo..").unwrap(), "foo");
        assert!(safe_profile_name("").is_err());
        assert!(safe_profile_name("___").is_err());
    }

    #[test]
    fn safe_profile_name_unicode_word_chars_pass() {
        // \w in regex matches Unicode letters with the `unicode-perl` feature
        // (regex crate default). 한글 should round-trip.
        assert_eq!(safe_profile_name("유저").unwrap(), "유저");
    }
}
