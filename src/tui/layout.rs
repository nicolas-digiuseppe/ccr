use std::collections::HashMap;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Sparkline},
    Frame,
};

use chrono::Datelike;

use super::list::format_duration;

use super::{App, Mode};

use crate::db::DayTokens;

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    if app.mode == Mode::Stats {
        render_dashboard(frame, app, area);
        return;
    }

    // Main vertical layout: search bar (4 lines) | body | status bar (1 line)
    let [search_area, body_area, status_area] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    // Render search bar
    super::search::render(frame, app, search_area);

    // Body: list + optional preview
    if app.show_preview {
        let [list_area, preview_area] = Layout::horizontal([
            Constraint::Percentage(40),
            Constraint::Percentage(60),
        ])
        .areas(body_area);

        super::list::render(frame, app, list_area);
        super::preview::render(frame, app, preview_area);
    } else {
        super::list::render(frame, app, body_area);
    }

    // Status bar
    render_status(frame, app, status_area);

    // Project filter popup overlay
    if app.mode == Mode::ProjectFilter {
        render_project_popup(frame, app, area);
    }

    // Tag input popup overlay
    if app.mode == Mode::TagInput {
        render_tag_input_popup(frame, app, area);
    }

    // Confirm delete popup overlay
    if app.mode == Mode::ConfirmDelete {
        render_confirm_delete_popup(frame, area);
    }

    // Help popup overlay
    if app.mode == Mode::Help {
        render_help_popup(frame, area);
    }
}

// ─── Full-screen stats dashboard ─────────────────────────────────────────────

fn render_dashboard(frame: &mut Frame, app: &App, area: Rect) {
    let stats = match &app.stats {
        Some(s) => s,
        None => {
            let para = Paragraph::new("Loading stats…")
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(para, area);
            return;
        }
    };

    // Vertical layout: search | metrics | activity bar | mid (weekly+cost) | bottom (breakdown+tokens) | status
    let [search_area, metrics_area, activity_area, mid_area, bottom_area, status_area] =
        Layout::vertical([
            Constraint::Length(4),   // header
            Constraint::Length(3),   // metrics bar
            Constraint::Length(4),   // heatmap (2 data rows + 2 borders)
            Constraint::Fill(1),     // weekly table + cost sparkline
            Constraint::Fill(1),     // token breakdown + tokens sparkline
            Constraint::Length(1),   // status
        ])
        .areas(area);

    // Search bar (shows "Dashboard" title in search.rs)
    super::search::render(frame, app, search_area);

    // ── Metrics bar ───────────────────────────────────────────────────────────
    render_metrics_bar(frame, app, metrics_area);

    // ── Activity bar ──────────────────────────────────────────────────────────
    render_activity_bar(frame, stats, activity_area);

    // ── Mid row: weekly table (50%) | cost sparkline (50%) ───────────────────
    let [table_area, cost_spark_area] = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Percentage(50),
    ])
    .areas(mid_area);

    render_weekly_table(frame, stats, table_area);
    render_cost_sparkline(frame, app, cost_spark_area);

    // ── Bottom row: token breakdown (40%) | token sparkline (60%) ────────────
    let [breakdown_area, token_spark_area] = Layout::horizontal([
        Constraint::Percentage(40),
        Constraint::Percentage(60),
    ])
    .areas(bottom_area);

    render_token_breakdown(frame, app, breakdown_area);
    render_tokens_sparkline(frame, app, token_spark_area);

    // ── Status bar ────────────────────────────────────────────────────────────
    let status_line = if stats.total_tokens == 0 && stats.total > 0 {
        Line::from(vec![
            Span::styled(" S", Style::default().fg(Color::Cyan)),
            Span::styled("/Esc", Style::default().fg(Color::DarkGray)),
            Span::styled(" return  ", Style::default().fg(Color::DarkGray)),
            Span::styled("│", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "  No token data — run ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled("ccr index --full", Style::default().fg(Color::Yellow)),
            Span::styled(" to populate costs", Style::default().fg(Color::DarkGray)),
        ])
    } else {
        let cost_summary = if stats.total_cost > 0.0 {
            format!(" · {} total", format_cost(stats.total_cost))
        } else {
            String::new()
        };
        let tokens_summary = if stats.total_tokens > 0 {
            format!(" · {} tokens", format_tokens(stats.total_tokens))
        } else {
            String::new()
        };
        Line::from(vec![
            Span::styled(" S", Style::default().fg(Color::Cyan)),
            Span::styled("/Esc", Style::default().fg(Color::DarkGray)),
            Span::styled(" return  ", Style::default().fg(Color::DarkGray)),
            Span::styled("│", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(
                    "  {} sessions{}{}",
                    format_num(stats.total),
                    tokens_summary,
                    cost_summary,
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    };
    frame.render_widget(Paragraph::new(status_line), status_area);
}

fn render_metrics_bar(frame: &mut Frame, app: &App, area: Rect) {
    let stats = match &app.stats {
        Some(s) => s,
        None => return,
    };

    // Total cost: prefer token data total, fall back to 0
    let total_cost_str = if stats.total_cost > 0.0 {
        format_cost(stats.total_cost)
    } else {
        "—".to_string()
    };

    let cache_hit = if stats.cache_hit_rate > 0.0 {
        format!("{:.1}%", stats.cache_hit_rate)
    } else {
        "—".to_string()
    };

    let avg_dur = format_duration(stats.avg_session_duration);

    let total_tokens_str = if stats.total_tokens > 0 {
        format_tokens(stats.total_tokens)
    } else {
        "—".to_string()
    };

    let metrics = [
        ("Sessions", format_num(stats.total), Color::Cyan),
        ("Tokens", total_tokens_str, Color::Green),
        ("Total Cost", total_cost_str, Color::Yellow),
        ("Cache Hit", cache_hit, Color::Magenta),
        ("Avg Duration", avg_dur, Color::Blue),
    ];

    let constraints: Vec<Constraint> = metrics.iter().map(|_| Constraint::Fill(1)).collect();
    let cells = Layout::horizontal(constraints).split(area);

    for (i, (label, value, color)) in metrics.iter().enumerate() {
        let block = Block::default()
            .title(Span::styled(
                format!(" {label} "),
                Style::default().fg(Color::DarkGray),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(cells[i]);
        frame.render_widget(block, cells[i]);

        let value_line = Line::from(Span::styled(
            value.as_str(),
            Style::default().fg(*color).add_modifier(Modifier::BOLD),
        ));
        let para = Paragraph::new(value_line)
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(para, inner);
    }
}

// ─── Activity bar ─────────────────────────────────────────────────────────────

fn render_activity_bar(frame: &mut Frame, stats: &crate::db::Stats, area: Rect) {
    let block = Block::default()
        .title(Span::styled(
            format!(" Activity — {} days ", stats.daily_sessions.len()),
            Style::default().fg(Color::DarkGray),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 1 || inner.width == 0 {
        return;
    }

    let width = inner.width as usize;
    let days = &stats.daily_sessions;

    // Take the rightmost `width` days (right = today, left = oldest)
    let slice: &[(String, u64)] = if days.len() > width {
        &days[days.len() - width..]
    } else {
        days
    };

    // Left-pad with empty days if fewer days than terminal width
    let pad = width.saturating_sub(slice.len());

    let mut spans: Vec<Span> = Vec::with_capacity(width);
    // Empty padding on the left
    for _ in 0..pad {
        spans.push(Span::styled("·", Style::default().fg(Color::DarkGray)));
    }
    // Actual data
    for (_, count) in slice {
        let (ch, color): (&'static str, Color) = match count {
            0 => ("·", Color::DarkGray),
            1 => ("░", Color::DarkGray),
            2..=3 => ("▒", Color::Green),
            4..=7 => ("▓", Color::LightGreen),
            _ => ("█", Color::White),
        };
        spans.push(Span::styled(ch, Style::default().fg(color)));
    }

    let row = Line::from(spans);
    // Render 2 identical rows for better visual weight
    let para = Paragraph::new(vec![row.clone(), row]);
    frame.render_widget(para, inner);
}

// ─── Weekly Summary table ─────────────────────────────────────────────────────

fn render_weekly_table(frame: &mut Frame, stats: &crate::db::Stats, area: Rect) {
    let block = Block::default()
        .title(Span::styled(" Weekly Summary ", Style::default().fg(Color::DarkGray)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    // Header
    lines.push(Line::from(vec![
        Span::styled(
            format!(" {:<10} {:>8} {:>10} {:>10}", "Week", "Sessions", "Tokens", "Cost"),
            Style::default().fg(Color::DarkGray),
        ),
    ]));

    // Build a map of date -> DayTokens
    let token_map: HashMap<&str, &DayTokens> = stats.daily_tokens.iter()
        .map(|dt| (dt.date.as_str(), dt))
        .collect();

    // Group by ISO week number, iterate in reverse (most recent first)
    let mut weeks: Vec<(String, u64, u64, f64)> = Vec::new(); // (week_label, sessions, tokens, cost)
    let mut current_week: Option<(String, u64, u64, f64)> = None;
    let mut prev_week_num: i32 = -1;
    let mut prev_year: i32 = -1;

    for (date_str, count) in stats.daily_sessions.iter().rev() {
        if let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            let week_num = date.iso_week().week() as i32;
            let year = date.iso_week().year();

            if week_num != prev_week_num || year != prev_year || current_week.is_none() {
                if let Some(w) = current_week.take() {
                    weeks.push(w);
                }
                // Label = Monday of this ISO week
                let monday = chrono::NaiveDate::from_isoywd_opt(year, week_num as u32, chrono::Weekday::Mon)
                    .unwrap_or(date);
                let label = monday.format("%b %d").to_string();
                current_week = Some((label, 0, 0, 0.0));
                prev_week_num = week_num;
                prev_year = year;
            }

            if let Some(ref mut w) = current_week {
                w.1 += count;
                if let Some(dt) = token_map.get(date_str.as_str()) {
                    w.2 += dt.total_tokens;
                    w.3 += dt.total_cost;
                }
            }
        }
    }
    if let Some(w) = current_week { weeks.push(w); }

    // Render rows (fit available height)
    let max_rows = inner.height.saturating_sub(1) as usize; // minus header
    for w in weeks.iter().take(max_rows) {
        let tokens_str = format_tokens(w.2);
        let cost_str = if w.3 > 0.0 { format!("${:.0}", w.3) } else { "–".to_string() };
        let sessions_str = format!("{}", w.1);

        let color = if w.1 > 0 { Color::White } else { Color::DarkGray };
        lines.push(Line::from(Span::styled(
            format!(" {:<10} {:>8} {:>10} {:>10}", w.0, sessions_str, tokens_str, cost_str),
            Style::default().fg(color),
        )));
    }

    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}

fn render_cost_sparkline(frame: &mut Frame, app: &App, area: Rect) {
    let stats = match &app.stats {
        Some(s) => s,
        None => return,
    };

    // Build last-30-days cost array from daily_tokens, padded to exactly 30 entries
    let today = chrono::Utc::now().date_naive();
    let inner_w = area.width.saturating_sub(2) as usize;
    let days = inner_w.max(30); // fill all available width, at least 30
    let mut cost_data: Vec<u64> = Vec::with_capacity(days);
    for day_offset in (0..days as i64).rev() {
        let date = today - chrono::Duration::days(day_offset);
        let date_str = date.format("%Y-%m-%d").to_string();
        let cost_cents = stats
            .daily_tokens
            .iter()
            .find(|d| d.date == date_str)
            .map(|d| (d.total_cost * 100.0).round() as u64)
            .unwrap_or(0);
        cost_data.push(cost_cents);
    }

    let max_cost_cents = cost_data.iter().copied().max().unwrap_or(0);
    let max_cost = max_cost_cents as f64 / 100.0;
    let avg_cost = if days > 0 {
        cost_data.iter().sum::<u64>() as f64 / 100.0 / days as f64
    } else {
        0.0
    };

    let title = if max_cost > 0.0 {
        format!(
            " Daily Cost · max {} · avg {}/day · {}d ",
            format_cost(max_cost),
            format_cost(avg_cost),
            days,
        )
    } else {
        format!(" Daily Cost · {}d ", days)
    };

    let sparkline = Sparkline::default()
        .data(&cost_data)
        .style(Style::default().fg(Color::Yellow))
        .block(
            Block::default()
                .title(Span::styled(title, Style::default().fg(Color::DarkGray)))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
    frame.render_widget(sparkline, area);
}

fn render_tokens_sparkline(frame: &mut Frame, app: &App, area: Rect) {
    let stats = match &app.stats {
        Some(s) => s,
        None => return,
    };

    // Build last-N-days token array (scale to thousands), padded to fill width
    let today = chrono::Utc::now().date_naive();
    let inner_w = area.width.saturating_sub(2) as usize;
    let days = inner_w.max(30);
    let mut token_data: Vec<u64> = Vec::with_capacity(days);
    for day_offset in (0..days as i64).rev() {
        let date = today - chrono::Duration::days(day_offset);
        let date_str = date.format("%Y-%m-%d").to_string();
        let tokens_k = stats
            .daily_tokens
            .iter()
            .find(|d| d.date == date_str)
            .map(|d| d.total_tokens / 1_000)
            .unwrap_or(0);
        token_data.push(tokens_k);
    }

    let max_tokens_k = token_data.iter().copied().max().unwrap_or(0);
    let max_tokens = max_tokens_k * 1_000;

    let title = if max_tokens > 0 {
        format!(
            " Daily Tokens · max {} · {}d ",
            format_tokens(max_tokens),
            days,
        )
    } else {
        format!(" Daily Tokens · {}d ", days)
    };

    let sparkline = Sparkline::default()
        .data(&token_data)
        .style(Style::default().fg(Color::Cyan))
        .block(
            Block::default()
                .title(Span::styled(title, Style::default().fg(Color::DarkGray)))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
    frame.render_widget(sparkline, area);
}

fn render_token_breakdown(frame: &mut Frame, app: &App, area: Rect) {
    let stats = match &app.stats {
        Some(s) => s,
        None => return,
    };

    let block = Block::default()
        .title(Span::styled(" Token Breakdown ", Style::default().fg(Color::DarkGray)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let total = stats.total_input_tokens + stats.total_output_tokens
        + stats.total_cache_creation_tokens + stats.total_cache_read_tokens;

    if total == 0 {
        let para = Paragraph::new(Line::from(Span::styled(
            "  No token data — run ccr index --full to populate",
            Style::default().fg(Color::DarkGray),
        )))
        .block(block);
        frame.render_widget(para, area);
        return;
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // " Label   " = 9 chars, "  94.5% " = 8 chars, remaining = bar space
    let label_col = 9usize;  // " Cache R "
    let pct_col = 8usize;    // "  94.5% "
    let bar_space = (inner.width as usize).saturating_sub(label_col + pct_col);

    let categories = [
        ("Input   ", stats.total_input_tokens, Color::Cyan),
        ("Output  ", stats.total_output_tokens, Color::Green),
        ("Cache W ", stats.total_cache_creation_tokens, Color::Yellow),
        ("Cache R ", stats.total_cache_read_tokens, Color::Magenta),
    ];

    let mut lines: Vec<Line> = Vec::new();

    for (label, count, color) in &categories {
        let pct = if total > 0 { *count as f64 / total as f64 } else { 0.0 };
        let bar_len = (pct * bar_space as f64).round() as usize;
        let bar_len = bar_len.min(bar_space).max(if pct > 0.0 { 1 } else { 0 });
        let pad_len = bar_space.saturating_sub(bar_len);
        let bar_str: String = "█".repeat(bar_len);
        let pad_str: String = " ".repeat(pad_len);
        let pct_str = format!("{:5.1}%", pct * 100.0);

        let line = Line::from(vec![
            Span::styled(format!(" {label}"), Style::default().fg(Color::DarkGray)),
            Span::styled(bar_str, Style::default().fg(*color)),
            Span::styled(pad_str, Style::default()),
            Span::styled(format!("  {pct_str}"), Style::default().fg(Color::DarkGray)),
        ]);
        lines.push(line);
    }

    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}

// ─── Format helpers ───────────────────────────────────────────────────────────

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1e9)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

fn format_cost(c: f64) -> String {
    if c >= 1000.0 {
        format!("${:.0}", c)
    } else if c >= 1.0 {
        format!("${:.2}", c)
    } else {
        format!("${:.3}", c)
    }
}

// ─── Status bar (normal mode) ─────────────────────────────────────────────────

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();

    if let Some(ref msg) = app.status_message {
        spans.push(Span::styled(format!(" {msg}  │ "), Style::default().fg(Color::Yellow)));
    } else {
        spans.push(Span::styled(" ", Style::default()));
    }

    let shortcuts = [
        ("/", "search "), ("⏎", "open "), ("y", "ank "), ("p", "roj "), ("s", "ort "),
        ("d", "ate "), ("e", "mpty "), ("*", "fav "), ("S", "tats "),
    ];
    for (key, label) in shortcuts {
        spans.push(Span::styled(key, Style::default().fg(Color::Cyan)));
        spans.push(Span::styled(label, Style::default().fg(Color::DarkGray)));
    }
    spans.push(Span::styled("?", Style::default().fg(Color::Cyan)));
    spans.push(Span::styled("help ", Style::default().fg(Color::DarkGray)));
    spans.push(Span::styled("q", Style::default().fg(Color::Red)));
    spans.push(Span::styled("uit", Style::default().fg(Color::DarkGray)));

    let text = Line::from(spans);
    let paragraph = Paragraph::new(text);
    frame.render_widget(paragraph, area);
}

fn render_confirm_delete_popup(frame: &mut Frame, area: Rect) {
    let popup_width = 36u16.min(area.width.saturating_sub(4));
    let popup_height = 5u16;

    let popup_x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_height)) / 2;

    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Delete Session? ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    let paragraph = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Delete this session? (y/N)",
            Style::default().fg(Color::White),
        )),
    ])
    .block(block);

    frame.render_widget(paragraph, popup_area);
}

fn render_tag_input_popup(frame: &mut Frame, app: &App, area: Rect) {
    let popup_width = 40u16.min(area.width.saturating_sub(4));
    let popup_height = 3u16;

    let popup_x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_height)) / 2;

    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(Clear, popup_area);

    let input_display = format!(" Tag: {}_", app.tag_input);
    let block = Block::default()
        .title(" Add Tag (Enter to confirm, Esc to cancel) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let paragraph = Paragraph::new(input_display)
        .block(block)
        .style(Style::default().fg(Color::White));

    frame.render_widget(paragraph, popup_area);
}

fn render_help_popup(frame: &mut Frame, area: Rect) {
    let popup_width = 52u16.min(area.width.saturating_sub(4));
    let popup_height = 26u16.min(area.height.saturating_sub(4));

    let popup_x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_height)) / 2;

    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(Span::styled(
            " Keybindings ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let binds: &[(&str, &str)] = &[
        ("Navigation", ""),
        ("  j/k  ↑/↓", "Move up/down"),
        ("  gg / G", "Jump to top / bottom"),
        ("  Ctrl+d/u", "Page down / up"),
        ("  Tab", "Toggle preview panel"),
        ("  J / K", "Scroll preview down / up"),
        ("", ""),
        ("Actions", ""),
        ("  Enter", "Open session"),
        ("  y", "Copy resume command"),
        ("  * / F", "Toggle favorite / filter favs"),
        ("  t", "Add tag"),
        ("  x", "Delete session"),
        ("", ""),
        ("Search & Filter", ""),
        ("  /", "Search"),
        ("  p", "Filter by project"),
        ("  d", "Cycle date filter"),
        ("  s", "Cycle sort mode"),
        ("  e", "Cycle empty filter"),
        ("  c", "Clear all filters"),
        ("", ""),
        ("  S", "Stats dashboard"),
        ("  r", "Re-index sessions"),
        ("  R", "Re-summarize (Ollama)"),
        ("  q / Esc", "Quit"),
    ];

    let max_rows = popup_height.saturating_sub(2) as usize;
    let lines: Vec<Line> = binds.iter().take(max_rows).map(|(key, desc)| {
        if desc.is_empty() && !key.is_empty() {
            // Section header
            Line::from(Span::styled(
                format!(" {key}"),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ))
        } else if key.is_empty() {
            Line::from("")
        } else {
            Line::from(vec![
                Span::styled(format!(" {key:<14}"), Style::default().fg(Color::White)),
                Span::styled(*desc, Style::default().fg(Color::DarkGray)),
            ])
        }
    }).collect();

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, popup_area);
}

fn format_num(n: i64) -> String {
    // Insert thousands separators
    let s = n.to_string();
    let mut result = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

fn render_project_popup(frame: &mut Frame, app: &mut App, area: Rect) {
    // Center a popup of roughly 40% width, up to 20 rows tall
    let popup_width = (area.width * 40 / 100).max(30).min(60);
    let popup_height = ((app.projects.len() as u16) + 4).min(20).min(area.height - 4);

    let popup_x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_height)) / 2;

    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    // Clear the popup area first
    frame.render_widget(Clear, popup_area);

    // Build list items: "All" + one per project
    let mut items: Vec<ListItem> = vec![ListItem::new(Line::from(Span::styled(
        "  (All projects)",
        Style::default().fg(Color::White),
    )))];

    for (name, count) in &app.projects {
        items.push(ListItem::new(Line::from(Span::styled(
            format!("  {name} ({count})"),
            Style::default().fg(Color::White),
        ))));
    }

    let block = Block::default()
        .title(" Filter by Project ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .style(Style::default().bg(Color::Reset));

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    let mut state = ListState::default();
    state.select(Some(app.project_selected));

    frame.render_stateful_widget(list, popup_area, &mut state);
}
