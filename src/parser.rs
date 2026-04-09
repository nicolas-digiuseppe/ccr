use std::path::Path;
use serde::Deserialize;
use serde_json::Value;

// ─── Public types ────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum MsgType {
    User,
    Assistant,
}

#[derive(Debug)]
pub struct ParsedMessage {
    pub msg_type: MsgType,
    pub text: String,
    pub timestamp: Option<String>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct ParsedSession {
    pub session_id: Option<String>,
    pub slug: Option<String>,
    pub first_message: String,
    pub summary: String,
    pub message_count: usize,
    pub first_timestamp: Option<String>,
    pub last_timestamp: Option<String>,
    pub duration_minutes: i64,
    pub fts_content: String,
    // Token usage (accumulated per-message from usage fields)
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_cost_usd: f64,
}

// ─── Internal deserialization structs ────────────────────────────────────────

#[derive(Deserialize)]
#[allow(dead_code)]
struct RawLine {
    #[serde(rename = "type")]
    line_type: String,
    message: Option<RawMessage>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct RawMessage {
    role: Option<String>,
    content: Option<Value>,
    model: Option<String>,
    usage: Option<RawUsage>,
}

// ─── Core parsing logic ───────────────────────────────────────────────────────

/// Check if text is system/meta content that should be skipped as first_message.
fn is_system_content(text: &str) -> bool {
    text.starts_with('<')
        || text.starts_with("Base directory for this skill:")
        || text.starts_with("Caveat:")
}

/// Check if text is injected/synthetic content that shouldn't be a first_message.
pub fn is_injected_content(text: &str) -> bool {
    is_system_content(text)
        || text.starts_with("Invoke the superpowers:")
        || text.starts_with("/superpowers:")
        || text.starts_with("Implement tasks from an OpenSpec")
        || text.starts_with("Tell your human partner")
        || text.starts_with("You are an impartial judge")
        || text.starts_with("Original premise:")
        || text == "Warmup"
        || text.starts_with("# Épreuve")
        || text.starts_with("Post qualification comment for ticket")
        || text.starts_with("Process Jira ticket")
        || text.starts_with("The user just ran")
        || text.starts_with("[Image")
        || text.starts_with("Sparring transcript")
        || text.starts_with("Tool loaded")
        || text.starts_with("[Request interrupted")
        || text.chars().count() < 4
        || text.starts_with("/Users/")
        || text.starts_with("~/")
        || text == "skip"
        || text == "continue"
        || text == "en français"
        || text.starts_with("Unknown skill:")
        || text.starts_with("Your shareable insights report")
        || text.starts_with("file:///")
}

/// Check if an assistant message is a generic/useless fallback.
pub fn is_generic_assistant(text: &str) -> bool {
    text.starts_with("```yaml")
        || text.starts_with("| Phase")
        || text.starts_with("I'm ready to help")
        || text.starts_with("I'll help you warm up")
        || text.starts_with("I'll perform a quick warmup")
        || text.starts_with("Hello! I'm Claude Code")
        || text.starts_with("I'll process")
        || text.starts_with("## Modèle de sécurité")
        || text.starts_with("I'll start by")
        || text.starts_with("Let me ")
}

/// Extract the best representative text from a content value.
/// Content can be a plain string or an array of typed blocks.
fn extract_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() || is_system_content(trimmed) {
                return None;
            }
            Some(trimmed.to_string())
        }
        Value::Array(blocks) => {
            // First pass: find text blocks >10 chars that don't start with `<`
            for block in blocks {
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        let trimmed = text.trim();
                        if trimmed.len() > 10 && !trimmed.starts_with('<') {
                            return Some(trimmed.to_string());
                        }
                    }
                }
            }
            // Second pass: accept short texts as fallback (still skip `<`-prefixed)
            for block in blocks {
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() && !trimmed.starts_with('<') {
                            return Some(trimmed.to_string());
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Collect ALL meaningful text for FTS indexing.
/// Skips thinking blocks and system content.
fn extract_all_text(content: &Value) -> Vec<String> {
    match content {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() || is_system_content(trimmed) {
                return vec![];
            }
            vec![trimmed.to_string()]
        }
        Value::Array(blocks) => {
            let mut texts = Vec::new();
            for block in blocks {
                let block_type = block.get("type").and_then(Value::as_str);
                if block_type == Some("thinking") || block_type == Some("tool_use") {
                    continue;
                }
                if block_type == Some("text") {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() && !trimmed.starts_with('<') {
                            texts.push(trimmed.to_string());
                        }
                    }
                }
            }
            texts
        }
        _ => vec![],
    }
}

/// Safe UTF-8 truncation.
fn truncate_utf8(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

/// Compute duration in minutes between two RFC3339 timestamps.
fn compute_duration(first: &str, last: &str) -> i64 {
    use chrono::DateTime;
    let t1 = DateTime::parse_from_rfc3339(first).ok();
    let t2 = DateTime::parse_from_rfc3339(last).ok();
    match (t1, t2) {
        (Some(a), Some(b)) => {
            let diff = b.signed_duration_since(a);
            diff.num_minutes()
        }
        _ => 0,
    }
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Parse a single JSONL line. Returns None for non-message lines or malformed JSON.
pub fn parse_jsonl_line(line: &str) -> Option<ParsedMessage> {
    let raw: RawLine = serde_json::from_str(line).ok()?;

    let msg_type = match raw.line_type.as_str() {
        "user" => MsgType::User,
        "assistant" => MsgType::Assistant,
        _ => return None,
    };

    let message = raw.message?;
    let content = message.content?;
    let text = extract_text(&content)?;

    Some(ParsedMessage {
        msg_type,
        text,
        timestamp: raw.timestamp,
    })
}

const FTS_CAP_CHARS: usize = 100_000;

/// Parse an entire session JSONL file into a ParsedSession.
pub fn parse_session(path: &Path) -> Option<ParsedSession> {
    let contents = std::fs::read_to_string(path).ok()?;

    let mut session_id: Option<String> = None;
    let mut slug: Option<String> = None;
    let mut first_message: Option<String> = None;
    let mut first_assistant: Option<String> = None;
    let mut message_count: usize = 0;
    let mut first_timestamp: Option<String> = None;
    let mut last_timestamp: Option<String> = None;
    let mut fts_parts: Vec<String> = Vec::new();
    let mut fts_total_chars: usize = 0;
    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut total_cache_creation: u64 = 0;
    let mut total_cache_read: u64 = 0;
    let mut total_cost_usd: f64 = 0.0;

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Try to extract sessionId and slug from any line
        if session_id.is_none() || slug.is_none() {
            if let Ok(val) = serde_json::from_str::<Value>(line) {
                if session_id.is_none() {
                    if let Some(sid) = val.get("sessionId").and_then(Value::as_str) {
                        session_id = Some(sid.to_string());
                    }
                }
                if slug.is_none() {
                    if let Some(s) = val.get("slug").and_then(Value::as_str) {
                        slug = Some(s.to_string());
                    }
                }
            }
        }

        if let Some(msg) = parse_jsonl_line(line) {
            message_count += 1;

            // Track timestamps
            if first_timestamp.is_none() {
                first_timestamp = msg.timestamp.clone();
            }
            if msg.timestamp.is_some() {
                last_timestamp = msg.timestamp.clone();
            }

            // Capture the first real user message (skip injected/skill content)
            if first_message.is_none() && msg.msg_type == MsgType::User && !is_injected_content(&msg.text) {
                first_message = Some(msg.text.clone());
            }
            // Fallback: first non-generic assistant message
            if first_assistant.is_none() && msg.msg_type == MsgType::Assistant && !is_generic_assistant(&msg.text) {
                first_assistant = Some(msg.text.clone());
            }

            // Accumulate FTS content (capped at 100KB)
            if fts_total_chars < FTS_CAP_CHARS {
                // Re-parse the line to get all text blocks for FTS
                if let Ok(raw) = serde_json::from_str::<RawLine>(line) {
                    if let Some(message) = raw.message {
                        if let Some(content) = message.content {
                            for t in extract_all_text(&content) {
                                let remaining = FTS_CAP_CHARS - fts_total_chars;
                                if remaining == 0 {
                                    break;
                                }
                                let chunk = truncate_utf8(&t, remaining);
                                fts_total_chars += chunk.chars().count();
                                fts_parts.push(chunk);
                            }
                        }
                    }
                }
            }

            // Accumulate token usage for assistant messages
            if let Ok(raw) = serde_json::from_str::<RawLine>(line) {
                if raw.line_type == "assistant" {
                    if let Some(ref msg) = raw.message {
                        if let Some(ref usage) = msg.usage {
                            let inp  = usage.input_tokens.unwrap_or(0);
                            let outp = usage.output_tokens.unwrap_or(0);
                            let cw   = usage.cache_creation_input_tokens.unwrap_or(0);
                            let cr   = usage.cache_read_input_tokens.unwrap_or(0);
                            total_input_tokens   += inp;
                            total_output_tokens  += outp;
                            total_cache_creation += cw;
                            total_cache_read     += cr;
                            let model = msg.model.as_deref().unwrap_or("");
                            total_cost_usd += crate::pricing::cost_usd(model, inp, outp, cw, cr);
                        }
                    }
                }
            }
        }
    }

    let first_message = first_message.or(first_assistant).unwrap_or_default();
    let summary = String::new(); // filled by Ollama via `ccr summarize`
    let duration_minutes = match (&first_timestamp, &last_timestamp) {
        (Some(a), Some(b)) if a != b => compute_duration(a, b),
        _ => 0,
    };

    let fts_content = {
        let mut parts = Vec::new();
        if !first_message.is_empty() {
            parts.push(first_message.clone());
        }
        parts.push(fts_parts.join(" "));
        parts.join("\n")
    };

    Some(ParsedSession {
        session_id,
        slug,
        first_message,
        summary,
        message_count,
        first_timestamp,
        last_timestamp,
        duration_minutes,
        fts_content,
        input_tokens: total_input_tokens,
        output_tokens: total_output_tokens,
        cache_creation_tokens: total_cache_creation,
        cache_read_tokens: total_cache_read,
        total_cost_usd,
    })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_user_message_string_content() {
        let line = r#"{"type":"user","message":{"role":"user","content":"hello world"},"timestamp":"2026-04-09T08:13:12.577Z"}"#;
        let msg = parse_jsonl_line(line).unwrap();
        assert_eq!(msg.msg_type, MsgType::User);
        assert_eq!(msg.text, "hello world");
        assert!(msg.timestamp.is_some());
    }

    #[test]
    fn test_parse_user_message_array_content() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"real question"},{"type":"text","text":"<system-reminder>skip me</system-reminder>"}]},"timestamp":"2026-04-09T10:00:00.000Z"}"#;
        let msg = parse_jsonl_line(line).unwrap();
        assert_eq!(msg.text, "real question");
    }

    #[test]
    fn test_skip_system_tags() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>ignored</system-reminder>"},{"type":"text","text":"actual message here"}]},"timestamp":"2026-04-09T10:00:00.000Z"}"#;
        let msg = parse_jsonl_line(line).unwrap();
        assert_eq!(msg.text, "actual message here");
    }

    #[test]
    fn test_skip_short_texts() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"ok"},{"type":"text","text":"this is the real message"}]},"timestamp":"2026-04-09T10:00:00.000Z"}"#;
        let msg = parse_jsonl_line(line).unwrap();
        assert_eq!(msg.text, "this is the real message");
    }

    #[test]
    fn test_parse_assistant_message() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I'll help you with that"}]},"timestamp":"2026-04-09T10:01:00.000Z"}"#;
        let msg = parse_jsonl_line(line).unwrap();
        assert_eq!(msg.msg_type, MsgType::Assistant);
        assert_eq!(msg.text, "I'll help you with that");
    }

    #[test]
    fn test_skip_string_system_content() {
        // String content that is a system tag should be skipped entirely
        let line = r#"{"type":"user","message":{"role":"user","content":"<local-command-caveat>The messages below were generated by the user while running local commands.</local-command-caveat>"},"timestamp":"2026-04-09T10:00:00.000Z"}"#;
        assert!(parse_jsonl_line(line).is_none());
    }

    #[test]
    fn test_skip_command_message() {
        let line = r#"{"type":"user","message":{"role":"user","content":"<command-message>review</command-message>"},"timestamp":"2026-04-09T10:00:00.000Z"}"#;
        assert!(parse_jsonl_line(line).is_none());
    }

    #[test]
    fn test_skip_skill_content() {
        let line = r#"{"type":"user","message":{"role":"user","content":"Base directory for this skill: /Users/test/.claude/skills/review\n\n# Review Workflow"},"timestamp":"2026-04-09T10:00:00.000Z"}"#;
        assert!(parse_jsonl_line(line).is_none());
    }

    #[test]
    fn test_session_skips_system_first_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.jsonl");
        std::fs::write(&path, concat!(
            r#"{"type":"user","message":{"role":"user","content":"<local-command-caveat>system stuff</local-command-caveat>"},"timestamp":"2026-04-09T08:00:00.000Z"}"#, "\n",
            r#"{"type":"user","message":{"role":"user","content":"actual user question here"},"timestamp":"2026-04-09T08:01:00.000Z"}"#, "\n",
        )).unwrap();

        let session = parse_session(&path).unwrap();
        assert_eq!(session.first_message, "actual user question here");
    }

    #[test]
    fn test_skip_thinking_blocks() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"internal thought"},{"type":"text","text":"visible response"}]},"timestamp":"2026-04-09T10:01:00.000Z"}"#;
        let msg = parse_jsonl_line(line).unwrap();
        assert_eq!(msg.text, "visible response");
    }

    #[test]
    fn test_malformed_json_returns_none() {
        let line = r#"{"broken json"#;
        assert!(parse_jsonl_line(line).is_none());
    }

    #[test]
    fn test_non_message_type_returns_none() {
        let line = r#"{"type":"permission-mode","permissionMode":"default"}"#;
        assert!(parse_jsonl_line(line).is_none());
    }

    #[test]
    fn test_session_skips_injected_first_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.jsonl");
        std::fs::write(&path, concat!(
            r#"{"type":"user","message":{"role":"user","content":"Base directory for this skill: /Users/test/.claude/skills/review\n\n# Review Workflow"},"timestamp":"2026-04-09T08:00:00.000Z"}"#, "\n",
            r#"{"type":"user","message":{"role":"user","content":"You are an impartial judge. Score this AI response against the ground truth."},"timestamp":"2026-04-09T08:00:01.000Z"}"#, "\n",
            r#"{"type":"user","message":{"role":"user","content":"review my PR please"},"timestamp":"2026-04-09T08:01:00.000Z"}"#, "\n",
        )).unwrap();

        let session = parse_session(&path).unwrap();
        assert_eq!(session.first_message, "review my PR please");
    }

    #[test]
    fn test_session_skips_warmup_and_skill_invokes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.jsonl");
        std::fs::write(&path, concat!(
            r#"{"type":"user","message":{"role":"user","content":"Warmup"},"timestamp":"2026-04-09T08:00:00.000Z"}"#, "\n",
            r#"{"type":"user","message":{"role":"user","content":"Invoke the superpowers:brainstorming skill"},"timestamp":"2026-04-09T08:00:01.000Z"}"#, "\n",
            r#"{"type":"user","message":{"role":"user","content":"build me a TUI for session browsing"},"timestamp":"2026-04-09T08:01:00.000Z"}"#, "\n",
        )).unwrap();

        let session = parse_session(&path).unwrap();
        assert_eq!(session.first_message, "build me a TUI for session browsing");
    }

    #[test]
    fn test_parse_session_accumulates_token_usage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.jsonl");
        std::fs::write(&path, concat!(
            r#"{"type":"user","message":{"role":"user","content":"hello"},"timestamp":"2026-04-13T10:00:00.000Z"}"#, "\n",
            r#"{"type":"assistant","message":{"role":"assistant","model":"claude-opus-4-6","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":100,"output_tokens":20,"cache_creation_input_tokens":500,"cache_read_input_tokens":1000}},"timestamp":"2026-04-13T10:01:00.000Z"}"#, "\n",
            r#"{"type":"user","message":{"role":"user","content":"follow up"},"timestamp":"2026-04-13T10:02:00.000Z"}"#, "\n",
            r#"{"type":"assistant","message":{"role":"assistant","model":"claude-opus-4-6","content":[{"type":"text","text":"sure"}],"usage":{"input_tokens":200,"output_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":2000}},"timestamp":"2026-04-13T10:03:00.000Z"}"#, "\n",
        )).unwrap();

        let session = parse_session(&path).unwrap();
        assert_eq!(session.input_tokens, 300);
        assert_eq!(session.output_tokens, 30);
        assert_eq!(session.cache_creation_tokens, 500);
        assert_eq!(session.cache_read_tokens, 3000);
        // Cost: opus-4 rates. 300 input @$15/M + 30 output @$75/M + 500 cache_write @$18.75/M + 3000 cache_read @$1.50/M
        let expected_cost = 300.0/1e6*15.0 + 30.0/1e6*75.0 + 500.0/1e6*18.75 + 3000.0/1e6*1.50;
        assert!((session.total_cost_usd - expected_cost).abs() < 0.0001,
            "got {}, expected {expected_cost}", session.total_cost_usd);
    }

    #[test]
    fn test_fts_content_includes_metadata() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"fix the login bug"}}]}},"sessionId":"abc123","timestamp":"2026-01-01T00:00:00Z"}}"#).unwrap();
        writeln!(f, r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"I'll fix the authentication issue"}}]}},"timestamp":"2026-01-01T00:01:00Z"}}"#).unwrap();

        let parsed = parse_session(&path).unwrap();
        assert!(parsed.fts_content.starts_with("fix the login bug\n"));
    }

    #[test]
    fn test_parse_session_zero_cost_when_no_usage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.jsonl");
        std::fs::write(&path, concat!(
            r#"{"type":"user","message":{"role":"user","content":"hello world"},"timestamp":"2026-04-13T10:00:00.000Z"}"#, "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"reply"}]},"timestamp":"2026-04-13T10:01:00.000Z"}"#, "\n",
        )).unwrap();

        let session = parse_session(&path).unwrap();
        assert_eq!(session.input_tokens, 0);
        assert_eq!(session.total_cost_usd, 0.0);
    }

    #[test]
    fn test_parse_session_extracts_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.jsonl");
        std::fs::write(&path, concat!(
            r#"{"type":"permission-mode","permissionMode":"default","sessionId":"abc-123"}"#, "\n",
            r#"{"type":"user","message":{"role":"user","content":"first real question"},"timestamp":"2026-04-09T08:00:00.000Z"}"#, "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"here is the answer"}]},"timestamp":"2026-04-09T08:05:00.000Z"}"#, "\n",
            r#"{"type":"user","message":{"role":"user","content":"follow up"},"timestamp":"2026-04-09T09:00:00.000Z"}"#, "\n",
        )).unwrap();

        let session = parse_session(&path).unwrap();
        assert_eq!(session.first_message, "first real question");
        assert_eq!(session.message_count, 3);
        assert_eq!(session.duration_minutes, 60);
        assert!(session.fts_content.contains("first real question"));
        assert!(session.fts_content.contains("here is the answer"));
    }
}
