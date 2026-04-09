use std::io::BufRead;
use crate::parser;

/// Extract the first N user+assistant messages from a JSONL file as context.
fn extract_context(jsonl_path: &str, max_messages: usize) -> String {
    let file = match std::fs::File::open(jsonl_path) {
        Ok(f) => f,
        Err(_) => return String::new(),
    };

    let reader = std::io::BufReader::new(file);
    let mut parts: Vec<String> = Vec::new();
    let mut count = 0;

    for line in reader.lines() {
        if count >= max_messages { break; }
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if let Some(msg) = parser::parse_jsonl_line(&line) {
            let role = match msg.msg_type {
                parser::MsgType::User => "User",
                parser::MsgType::Assistant => "Assistant",
            };
            let text: String = msg.text.chars().take(200).collect();
            parts.push(format!("{role}: {text}"));
            count += 1;
        }
    }

    parts.join("\n")
}

/// Call Ollama locally to generate a one-line summary.
fn summarize_via_ollama(context: &str) -> Option<String> {
    let body = serde_json::json!({
        "model": "qwen2.5:7b",
        "prompt": format!(
            "TASK: Write a max 10-word summary of what was BUILT or DONE in this coding session. Be specific (mention file names, features, bugs). Output ONLY the summary, no prefix, no quotes.\n\n{context}"
        ),
        "stream": false,
        "options": { "num_predict": 30, "temperature": 0.1 }
    });

    let resp = std::process::Command::new("curl")
        .args(["-s", "http://localhost:11434/api/generate", "-d", &body.to_string()])
        .output()
        .ok()?;

    if !resp.status.success() { return None; }

    let val: serde_json::Value = serde_json::from_slice(&resp.stdout).ok()?;
    let text = val.get("response")?.as_str()?.trim().to_string();

    // Clean up: remove quotes, markdown, prefixes, take first line only
    let mut clean = text.trim_matches('"').lines().next().unwrap_or("").trim().to_string();

    // Strip common prefixes the model adds
    for prefix in &["Summary:", "**Summary:**", "summary:", "Here is", "Here's"] {
        if let Some(rest) = clean.strip_prefix(prefix) {
            clean = rest.trim().to_string();
        }
    }
    // Strip markdown bold
    clean = clean.replace("**", "");
    // Truncate to 80 chars
    if clean.chars().count() > 80 {
        clean = clean.chars().take(77).collect::<String>() + "...";
    }

    if clean.is_empty() || clean.len() < 5 { None } else { Some(clean) }
}

/// Summarize a single session. Returns None if Ollama is unavailable or context is empty.
pub fn summarize_session(jsonl_path: &str) -> Option<String> {
    let context = extract_context(jsonl_path, 8);
    if context.is_empty() { return None; }
    summarize_via_ollama(&context)
}

/// Batch summarize all sessions without a summary. Returns (done, skipped, errors).
pub fn batch_summarize(db: &crate::db::Database) -> (usize, usize, usize) {
    // Check Ollama is running
    let check = std::process::Command::new("curl")
        .args(["-s", "http://localhost:11434/api/version"])
        .output();
    match check {
        Ok(o) if o.status.success() => {}
        _ => {
            eprintln!("Error: Ollama is not running. Start it with: brew services start ollama");
            return (0, 0, 0);
        }
    }

    let sessions = db.sessions_needing_summary().unwrap_or_default();
    let total = sessions.len();
    if total == 0 {
        println!("All sessions already have summaries.");
        return (0, 0, 0);
    }

    let mut done = 0;
    let mut skipped = 0;
    let mut errors = 0;

    for (i, (session_id, jsonl_path)) in sessions.iter().enumerate() {
        eprint!("\r  [{}/{}] Summarizing...", i + 1, total);
        match summarize_session(jsonl_path) {
            Some(summary) => {
                let _ = db.set_summary(session_id, &summary);
                done += 1;
            }
            None => {
                skipped += 1;
            }
        }
    }
    if total > 0 { eprintln!(); }

    (done, skipped, errors)
}
