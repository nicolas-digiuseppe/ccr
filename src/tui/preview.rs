use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use super::App;

fn strip_inline_md(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' | '_' => { if chars.peek() == Some(&ch) { chars.next(); } }
            '`' => { while chars.peek() == Some(&'`') { chars.next(); } }
            '[' => {
                let mut t = String::new();
                for c in chars.by_ref() { if c == ']' { break; } t.push(c); }
                if chars.peek() == Some(&'(') {
                    chars.next();
                    for c in chars.by_ref() { if c == ')' { break; } }
                }
                result.push_str(&t);
            }
            _ => result.push(ch),
        }
    }
    result
}

fn strip_markdown(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut in_code_block = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") { in_code_block = !in_code_block; continue; }
        if in_code_block { lines.push(format!("    {line}")); continue; }
        if trimmed == "---" || trimmed == "***" || trimmed == "___" { continue; }
        let processed = if let Some(rest) = trimmed.strip_prefix("### ") {
            rest.to_uppercase()
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            rest.to_uppercase()
        } else if let Some(rest) = trimmed.strip_prefix("# ") {
            rest.to_uppercase()
        } else {
            strip_inline_md(line)
        };
        lines.push(processed);
    }
    lines.join("\n")
}

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    if app.preview_messages.is_empty() {
        lines.push(Line::from(Span::styled(
            " Select a session",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for (idx, (role, text)) in app.preview_messages.iter().enumerate() {
            if role == "separator" {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!(" {text}"),
                    Style::default().fg(Color::DarkGray),
                )));
                if idx < app.preview_messages.len() - 1 {
                    lines.push(Line::from(""));
                }
                continue;
            }

            let (label, label_fg) = if role == "user" {
                (" you ", Color::Cyan)
            } else {
                (" claude ", Color::Green)
            };

            lines.push(Line::from(vec![
                Span::styled(label, Style::default().fg(Color::Black).bg(label_fg).add_modifier(Modifier::BOLD)),
            ]));

            let cleaned = strip_markdown(text);
            let display: String = cleaned.chars().take(1000).collect();
            let truncated = cleaned.chars().count() > 1000;

            let msg_fg = if role == "user" { Color::White } else { Color::Gray };
            for msg_line in display.lines() {
                lines.push(Line::from(Span::styled(
                    format!(" {msg_line}"),
                    Style::default().fg(msg_fg),
                )));
            }
            if truncated {
                lines.push(Line::from(Span::styled(" …", Style::default().fg(Color::DarkGray))));
            }

            if idx < app.preview_messages.len() - 1 {
                lines.push(Line::from(""));
            }
        }
    }

    let block = Block::default()
        .title(Span::styled(" preview ", Style::default().fg(Color::DarkGray)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.preview_scroll, 0));

    frame.render_widget(paragraph, area);
}
