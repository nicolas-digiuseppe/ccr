use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};

use super::App;

#[allow(dead_code)]
pub fn format_size(bytes: i64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1}MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1_024 {
        format!("{:.0}KB", bytes as f64 / 1_024.0)
    } else {
        format!("{bytes}B")
    }
}

pub fn format_duration(minutes: i64) -> String {
    if minutes <= 0 {
        return "–".to_string();
    }
    let hours = minutes / 60;
    let mins = minutes % 60;
    if hours > 0 {
        if mins > 0 { format!("{hours}h{mins:02}") } else { format!("{hours}h") }
    } else {
        format!("{mins}m")
    }
}

fn format_date(last_modified: &str) -> String {
    let dt = if let Ok(ts) = last_modified.parse::<i64>() {
        chrono::DateTime::from_timestamp(ts, 0).map(|d| d.with_timezone(&chrono::Utc))
    } else {
        chrono::DateTime::parse_from_rfc3339(last_modified)
            .ok()
            .map(|d| d.with_timezone(&chrono::Utc))
    };

    match dt {
        Some(d) => d.format("%b %d %H:%M").to_string(), // "Apr 09 14:30"
        None => last_modified.chars().take(12).collect(),
    }
}

fn strip_md(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' | '_' => { if chars.peek() == Some(&ch) { chars.next(); } }
            '`' => { while chars.peek() == Some(&'`') { chars.next(); } }
            '#' => {
                if result.is_empty() || result.ends_with('\n') {
                    while chars.peek() == Some(&'#') || chars.peek() == Some(&' ') {
                        if chars.peek() == Some(&' ') { chars.next(); break; }
                        chars.next();
                    }
                } else { result.push(ch); }
            }
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

fn clean_message(msg: &str, max: usize) -> String {
    let trimmed = msg.trim();
    if trimmed.is_empty() { return "·".to_string(); }
    let first_line = trimmed.lines().find(|l| !l.trim().is_empty()).unwrap_or(trimmed);
    let cleaned = strip_md(first_line.trim());
    let display: String = cleaned.chars().take(max).collect();
    if cleaned.chars().count() > max { format!("{display}…") } else { display }
}

// Terminal-native colors — respects user's theme
const SEL_BG: Color = Color::DarkGray;
const SEL_FG: Color = Color::White;

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let sessions = &app.filtered_sessions;
    let selected = app.selected;
    let width = area.width.saturating_sub(4) as usize;

    let items: Vec<ListItem> = sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let date_str = format_date(&s.last_modified);
            let dur_str = format_duration(s.duration_minutes);
            let is_selected = i == selected;

            let star = if app.favorites.contains(&s.session_id) { "★ " } else { "" };
            let right = format!("{} · {}", s.message_count, dur_str);
            let left_len = date_str.chars().count() + 2 + star.chars().count() + s.project.chars().count();
            let padding = width.saturating_sub(left_len + right.chars().count());
            let pad_str = " ".repeat(padding);

            let line1 = if is_selected {
                Line::from(vec![
                    Span::styled(date_str, Style::default().fg(Color::Gray)),
                    Span::styled("  ", Style::default()),
                    Span::styled(star.to_string(), Style::default().fg(Color::Yellow)),
                    Span::styled(s.project.clone(), Style::default().fg(SEL_FG).add_modifier(Modifier::BOLD)),
                    Span::styled(pad_str, Style::default()),
                    Span::styled(right, Style::default().fg(Color::Gray)),
                ])
            } else {
                Line::from(vec![
                    Span::styled(date_str, Style::default().fg(Color::DarkGray)),
                    Span::styled("  ", Style::default()),
                    Span::styled(star.to_string(), Style::default().fg(Color::Yellow)),
                    Span::styled(s.project.clone(), Style::default().fg(Color::Cyan)),
                    Span::styled(pad_str, Style::default()),
                    Span::styled(right, Style::default().fg(Color::DarkGray)),
                ])
            };

            let has_ai_summary = s.summary.as_deref().is_some_and(|s| !s.is_empty());
            let display_text = if has_ai_summary {
                s.summary.as_deref().unwrap()
            } else {
                &s.first_message
            };
            let msg_max = width.saturating_sub(4);
            let msg = clean_message(display_text, msg_max);
            let msg_fg = if is_selected { Color::White } else if has_ai_summary { Color::Gray } else { Color::DarkGray };
            let prefix = if has_ai_summary { "  " } else { "  ░ " };
            let line2 = Line::from(vec![
                Span::styled(format!("{prefix}{msg}"), Style::default().fg(msg_fg)),
            ]);

            ListItem::new(vec![line1, line2])
        })
        .collect();

    let title = format!(" {} sessions ", sessions.len());
    let block = Block::default()
        .title(Span::styled(title, Style::default().fg(Color::White)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(SEL_BG))
        .highlight_symbol("▸ ");

    let mut state = ListState::default();
    if !sessions.is_empty() { state.select(Some(selected)); }
    frame.render_stateful_widget(list, area, &mut state);
}
