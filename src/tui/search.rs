use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::{App, Mode};


/// Format "YYYY-MM-DD" to "Mon DD" (e.g. "2026-01-09" → "Jan 09").
fn fmt_date_short(iso: &str) -> String {
    let parts: Vec<&str> = iso.split('-').collect();
    if parts.len() != 3 { return iso.to_string(); }
    let month = match parts[1] {
        "01" => "Jan", "02" => "Feb", "03" => "Mar", "04" => "Apr",
        "05" => "May", "06" => "Jun", "07" => "Jul", "08" => "Aug",
        "09" => "Sep", "10" => "Oct", "11" => "Nov", "12" => "Dec",
        _ => "???",
    };
    format!("{} {}", month, parts[2])
}

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    // In Stats mode show a simplified dashboard header
    if app.mode == Mode::Stats {
        let block = Block::default()
            .title(Span::styled(
                " ccr ",
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        // Build date range subtitle from stats
        let subtitle = if let Some(stats) = &app.stats {
            if !stats.first_date.is_empty() && !stats.last_date.is_empty() {
                // compute number of days between first and last
                let days = chrono::NaiveDate::parse_from_str(&stats.first_date, "%Y-%m-%d")
                    .ok()
                    .zip(chrono::NaiveDate::parse_from_str(&stats.last_date, "%Y-%m-%d").ok())
                    .map(|(a, b)| (b - a).num_days() + 1)
                    .unwrap_or(0);
                format!(
                    "  Usage statistics · {} – {} ({} days)",
                    fmt_date_short(&stats.first_date),
                    fmt_date_short(&stats.last_date),
                    days,
                )
            } else {
                "  Usage statistics for all Claude Code sessions".to_string()
            }
        } else {
            "  Usage statistics for all Claude Code sessions".to_string()
        };

        let para = Paragraph::new(vec![
            Line::from(Span::styled(
                " Dashboard",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(subtitle, Style::default().fg(Color::DarkGray))),
        ])
        .block(block);
        frame.render_widget(para, area);
        return;
    }

    let is_searching = app.mode == Mode::Search;

    let badge_label = " / ";
    let badge_fg = Color::Cyan;

    // Input line
    let mut spans = vec![
        Span::styled(badge_label, Style::default().fg(Color::Black).bg(badge_fg).add_modifier(Modifier::BOLD)),
        Span::styled(" ", Style::default()),
    ];

    if app.search_input.is_empty() && !is_searching {
        spans.push(Span::styled("Search sessions…", Style::default().fg(Color::DarkGray)));
    } else {
        spans.push(Span::styled(app.search_input.clone(), Style::default().fg(Color::White)));
        if is_searching {
            spans.push(Span::styled("▌", Style::default().fg(badge_fg)));
        }
    }

    let input_line = Line::from(spans);

    // Active filters as compact badges
    let mut filters: Vec<Span> = vec![Span::styled("  ", Style::default())];
    let mut has_filter = false;

    if let Some(ref proj) = app.project_filter {
        filters.push(Span::styled(format!(" {proj} "), Style::default().fg(Color::Black).bg(Color::Yellow)));
        filters.push(Span::raw(" "));
        has_filter = true;
    }
    if app.date_filter != super::DateFilter::All {
        filters.push(Span::styled(format!(" {} ", app.date_filter.label()), Style::default().fg(Color::Black).bg(Color::Green)));
        filters.push(Span::raw(" "));
        has_filter = true;
    }
    if app.sort_mode != super::SortMode::Date {
        filters.push(Span::styled(format!(" ↕ {} ", app.sort_mode.label()), Style::default().fg(Color::Black).bg(Color::Blue)));
        filters.push(Span::raw(" "));
        has_filter = true;
    }
    if app.show_favorites_only {
        filters.push(Span::styled(" ★ ", Style::default().fg(Color::Black).bg(Color::Yellow)));
        filters.push(Span::raw(" "));
        has_filter = true;
    }
    let empty_label = app.empty_filter.label();
    if !empty_label.is_empty() {
        filters.push(Span::styled(format!(" {empty_label} "), Style::default().fg(Color::Black).bg(Color::Red)));
        filters.push(Span::raw(" "));
        has_filter = true;
    }
    if !has_filter {
        filters.push(Span::styled("no filters", Style::default().fg(Color::DarkGray)));
    }

    let filter_line = Line::from(filters);

    let border_color = if is_searching { badge_fg } else { Color::DarkGray };
    let block = Block::default()
        .title(Span::styled(" ccr ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let paragraph = Paragraph::new(vec![input_line, filter_line]).block(block);
    frame.render_widget(paragraph, area);
}
