use std::path::{Path, PathBuf};
use anyhow::Result;
use crate::db::{Database, SessionRow};
use crate::parser;

// ─── Public types ─────────────────────────────────────────────────────────────

pub struct ScanEntry {
    pub session_id: String,
    pub jsonl_path: PathBuf,
    pub project_dir_name: String,
    pub project_display: String,
    pub project_path: String,
}

// ─── Path decoding ─────────────────────────────────────────────────────────────

/// Backtracking decode of an encoded Claude project directory name.
///
/// Claude encodes project paths by replacing `/` with `-` (with a leading `-`
/// for absolute paths). For example `/Users/nick/www/skello-app-front` becomes
/// `-Users-nick-www-skello-app-front`.
///
/// The decode strategy:
/// 1. Strip the leading `-` and replace it with `/`.
/// 2. Split the rest on `-`.
/// 3. Use backtracking to try all possible groupings of segments (slash vs dash join)
///    and return the first combination that corresponds to a real directory.
/// 4. Fallback: keep as many trailing tokens dash-joined (up to a reasonable depth),
///    using the last known-existing directory prefix.
///
/// Returns `(display_name, canonical_path)` where `display_name` is the last
/// path component and `canonical_path` is the best guess at the full path.
pub fn decode_project_dir(encoded: &str) -> (String, String) {
    // Encoded paths start with a `-` for the leading `/`.
    let without_leading = if encoded.starts_with('-') {
        &encoded[1..]
    } else {
        encoded
    };

    // Split on `-` to get candidate segments.
    let segments: Vec<&str> = without_leading.split('-').collect();

    // Try to reconstruct a real path using backtracking.
    if let Some(path) = backtrack(&segments, 0, "/") {
        let display = Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        return (display, path);
    }

    // Fallback: find the longest prefix that exists, then dash-join the remainder.
    let fallback = fallback_path(&segments);
    let display = Path::new(&fallback)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| fallback.clone());

    (display, fallback)
}

/// Recursive backtracking over `segments[idx..]`, building on `prefix`.
///
/// At each step we try all possible lengths of the next component:
/// - single segment (slash-join)
/// - two dash-joined segments
/// - three dash-joined segments
/// - etc.
///
/// We prefer the shortest component first (slash-join preference) and recurse.
/// A candidate is accepted only if it exists on disk.
fn backtrack(segments: &[&str], idx: usize, prefix: &str) -> Option<String> {
    if idx == segments.len() {
        // All segments consumed — check that the accumulated path actually exists.
        if Path::new(prefix).exists() {
            return Some(prefix.to_string());
        }
        return None;
    }

    // Try taking 1..=remaining segments as the next component, dash-joined.
    // Claude Code also encodes dots as dashes, so try dot-joined variants too.
    let remaining = segments.len() - idx;
    for len in 1..=remaining {
        let dash_joined = segments[idx..idx + len].join("-");
        let variants = if len > 1 {
            vec![dash_joined.clone(), segments[idx..idx + len].join(".")]
        } else {
            vec![dash_joined]
        };

        for component in &variants {
            let candidate = if prefix == "/" {
                format!("/{}", component)
            } else {
                format!("{}/{}", prefix, component)
            };

            if !Path::new(&candidate).exists() {
                continue;
            }

            if let Some(result) = backtrack(segments, idx + len, &candidate) {
                return Some(result);
            }
        }

        if len == remaining {
            break;
        }
    }

    None
}

/// Fallback path when no on-disk match is found.
/// Finds the longest prefix that exists, then dash-joins all remaining segments.
fn fallback_path(segments: &[&str]) -> String {
    let mut best_prefix = String::from("/");
    let mut best_idx = 0;

    // Walk segment by segment (slash-join), finding the deepest existing prefix.
    let mut current = String::from("/");
    for (i, seg) in segments.iter().enumerate() {
        let candidate = if current == "/" {
            format!("/{}", seg)
        } else {
            format!("{}/{}", current, seg)
        };
        if Path::new(&candidate).exists() {
            best_prefix = candidate.clone();
            best_idx = i + 1;
            current = best_prefix.clone();
        } else {
            break;
        }
    }

    // Dash-join all remaining segments after the best prefix.
    if best_idx < segments.len() {
        let remainder = segments[best_idx..].join("-");
        if best_prefix == "/" {
            format!("/{}", remainder)
        } else {
            format!("{}/{}", best_prefix, remainder)
        }
    } else {
        best_prefix
    }
}

// ─── Session ID extraction ────────────────────────────────────────────────────

/// Extract the session ID from a JSONL file path (the file stem).
pub fn session_id_from_path(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

// ─── Directory scanning ───────────────────────────────────────────────────────

/// Scan `projects_dir` for all `.jsonl` session files.
/// Returns one `ScanEntry` per `.jsonl` file found.
pub fn scan_projects_dir(projects_dir: &Path) -> Vec<ScanEntry> {
    let mut entries = Vec::new();

    let read_dir = match std::fs::read_dir(projects_dir) {
        Ok(rd) => rd,
        Err(_) => return entries,
    };

    for dir_entry in read_dir.flatten() {
        let dir_path = dir_entry.path();
        if !dir_path.is_dir() {
            continue;
        }

        let dir_name = dir_entry.file_name().to_string_lossy().to_string();
        let (project_display, project_path) = decode_project_dir(&dir_name);

        let jsonl_read = match std::fs::read_dir(&dir_path) {
            Ok(rd) => rd,
            Err(_) => continue,
        };

        for file_entry in jsonl_read.flatten() {
            let file_path = file_entry.path();
            if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            let session_id = session_id_from_path(&file_path);
            entries.push(ScanEntry {
                session_id,
                jsonl_path: file_path,
                project_dir_name: dir_name.clone(),
                project_display: project_display.clone(),
                project_path: project_path.clone(),
            });
        }
    }

    entries
}

// ─── Incremental indexer ──────────────────────────────────────────────────────

/// Index all sessions found under `projects_dir` into `db`.
///
/// - If `full` is true, re-index every session regardless of mtime.
/// - Otherwise, skip sessions whose stored mtime matches the file's mtime.
/// - Projects in `exclude_projects` are skipped entirely.
/// - Returns `(indexed_count, total_count)`.
pub fn run_index(
    db: &Database,
    projects_dir: &Path,
    full: bool,
    exclude_projects: &[String],
) -> Result<(u32, u32)> {
    let entries = scan_projects_dir(projects_dir);

    let total = entries.len() as u32;
    let mut indexed = 0u32;

    for entry in &entries {
        // Skip excluded projects (match against display name and dir name).
        if exclude_projects
            .iter()
            .any(|ex| ex == &entry.project_display || ex == &entry.project_dir_name)
        {
            continue;
        }

        // Get file mtime as unix epoch seconds string.
        let file_mtime = std::fs::metadata(&entry.jsonl_path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs().to_string())
            });

        // Incremental: skip if mtime hasn't changed.
        if !full {
            if let Some(ref file_mt) = file_mtime {
                if let Some(stored_mt) = db.get_session_mtime(&entry.session_id)? {
                    if &stored_mt == file_mt {
                        continue;
                    }
                }
            }
        }

        // Parse the session.
        let parsed = match parser::parse_session(&entry.jsonl_path) {
            Some(p) => p,
            None => continue,
        };

        // Build the DB row.
        // Use the file mtime as last_modified for incremental caching; fall back
        // to the last timestamp from the parsed session if no mtime available.
        let last_modified = file_mtime
            .clone()
            .or_else(|| parsed.last_timestamp.clone())
            .unwrap_or_default();

        let file_size = std::fs::metadata(&entry.jsonl_path)
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        let row = SessionRow {
            session_id: entry.session_id.clone(),
            project: entry.project_display.clone(),
            project_path: entry.project_path.clone(),
            jsonl_path: entry.jsonl_path.to_string_lossy().to_string(),
            first_message: parsed.first_message,
            summary: parsed.summary,
            slug: parsed.slug,
            started_at: parsed.first_timestamp.unwrap_or_default(),
            last_modified,
            message_count: parsed.message_count as i64,
            file_size,
            duration_minutes: parsed.duration_minutes,
            fts_content: parsed.fts_content,
            input_tokens: parsed.input_tokens,
            output_tokens: parsed.output_tokens,
            cache_creation_tokens: parsed.cache_creation_tokens,
            cache_read_tokens: parsed.cache_read_tokens,
            total_cost_usd: parsed.total_cost_usd,
        };

        db.upsert_session(&row)?;
        indexed += 1;
    }

    // Clean up sessions whose files have been deleted.
    db.purge_missing()?;

    Ok((indexed, total))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_project_path_simple() {
        let encoded = "-tmp-myapp";
        let (display, _) = decode_project_dir(encoded);
        assert_eq!(display, "myapp");
    }

    #[test]
    fn test_decode_project_path_with_dashes() {
        let encoded = "-tmp-my-cool-app";
        let (display, _) = decode_project_dir(encoded);
        assert!(display.contains("my"));
    }

    #[test]
    fn test_extract_session_id_from_filename() {
        let path = Path::new("/home/.claude/projects/proj/abc-def-123.jsonl");
        let id = session_id_from_path(path);
        assert_eq!(id, "abc-def-123");
    }

    #[test]
    fn test_decode_dotted_username() {
        // Claude Code encodes dots as dashes (e.g. user.name -> user-name)
        // Create a temp dir simulating /Users/user.name/project
        let dir = tempfile::tempdir().unwrap();
        let user_dir = dir.path().join("user.name");
        let project_dir = user_dir.join("project");
        std::fs::create_dir_all(&project_dir).unwrap();

        let dir_str = dir.path().to_string_lossy();
        // Simulate encoded path: strip leading / and replace / and . with -
        let encoded = format!("-{}-user-name-project", dir_str.trim_start_matches('/').replace('/', "-"));
        let (display, path) = decode_project_dir(&encoded);
        assert_eq!(display, "project");
        assert_eq!(path, format!("{}/user.name/project", dir_str));
    }

    #[test]
    fn test_scan_projects_dir_with_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = dir.path().join("test-project");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(
            project_dir.join("session-001.jsonl"),
            r#"{"type":"user","message":{"role":"user","content":"hello"},"timestamp":"2026-04-09T08:00:00.000Z"}"#,
        ).unwrap();
        let entries = scan_projects_dir(dir.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id, "session-001");
    }
}
