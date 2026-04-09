use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use crate::indexer;
use super::{App, Mode, SortMode};

pub fn handle_key_with_modifiers(app: &mut App, key: KeyCode, modifiers: KeyModifiers) {
    match app.mode {
        Mode::Normal => handle_normal(app, key, modifiers),
        Mode::Search => handle_search(app, key),
        Mode::ProjectFilter => handle_project_filter(app, key),
        Mode::TagInput => handle_tag_input(app, key),
        Mode::ConfirmDelete => handle_confirm_delete(app, key),
        Mode::Stats => handle_stats(app, key),
        Mode::Help => handle_help(app, key),
    }
}

fn handle_normal(app: &mut App, key: KeyCode, modifiers: KeyModifiers) {
    match key {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.should_quit = true;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if !app.filtered_sessions.is_empty() {
                app.selected = (app.selected + 1).min(app.filtered_sessions.len() - 1);
                app.load_preview();
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.selected > 0 {
                app.selected -= 1;
                app.load_preview();
            }
        }
        // Ctrl+d: page down (half screen)
        KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
            if !app.filtered_sessions.is_empty() {
                let page = app.visible_height / 2;
                app.selected = (app.selected + page as usize).min(app.filtered_sessions.len() - 1);
                app.load_preview();
            }
        }
        // Ctrl+u: page up (half screen)
        KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
            if !app.filtered_sessions.is_empty() {
                let page = app.visible_height / 2;
                app.selected = app.selected.saturating_sub(page as usize);
                app.load_preview();
            }
        }
        // G: go to bottom
        KeyCode::Char('G') => {
            if !app.filtered_sessions.is_empty() {
                app.selected = app.filtered_sessions.len() - 1;
                app.load_preview();
            }
        }
        // g: first press sets pending_g, second press (gg) goes to top
        KeyCode::Char('g') => {
            if app.pending_g {
                app.selected = 0;
                app.load_preview();
                app.pending_g = false;
            } else {
                app.pending_g = true;
            }
        }
        KeyCode::Char('/') => {
            app.mode = Mode::Search;
        }
        KeyCode::Char('p') => {
            app.mode = Mode::ProjectFilter;
        }
        KeyCode::Char('d') => {
            app.date_filter = app.date_filter.next();
            app.apply_filters();
        }
        KeyCode::Char('s') => {
            app.sort_mode = app.sort_mode.next();
            app.apply_filters();
        }
        KeyCode::Char('e') => {
            app.empty_filter = app.empty_filter.next();
            app.set_status(match app.empty_filter {
                super::EmptyFilter::HideEmpty => "Hiding empty sessions".to_string(),
                super::EmptyFilter::ShowAll => "Showing all sessions".to_string(),
                super::EmptyFilter::OnlyEmpty => "Showing empty sessions only".to_string(),
            });
            app.apply_filters();
        }
        KeyCode::Char('c') => {
            app.search_input.clear();
            app.project_filter = None;
            app.date_filter = super::DateFilter::All;
            app.sort_mode = SortMode::Date;
            app.apply_filters();
        }
        KeyCode::Char('r') | KeyCode::Char('i') => {
            app.set_status("Re-indexing...".to_string());
            let projects_dir = dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                .join(".claude/projects");
            let exclude = app.config.exclude_projects.clone();
            // Spawn re-index in background — we can't easily pass db across threads
            // so we do a quick synchronous re-index on the same db connection
            let db_path = crate::db::Database::default_path();
            std::thread::spawn(move || {
                if let Ok(db) = crate::db::Database::open(&db_path) {
                    let _ = indexer::run_index(&db, &projects_dir, false, &exclude);
                }
            });
        }
        KeyCode::Tab => {
            app.show_preview = !app.show_preview;
        }
        KeyCode::Enter => {
            if let Some(session) = app.filtered_sessions.get(app.selected) {
                app.launch = Some(crate::launcher::build_launch_request(
                    &app.config.launch_command,
                    &session.session_id,
                    &session.project_path,
                ));
                app.should_quit = true;
            }
        }
        // Shift+J: preview scroll down
        KeyCode::Char('J') => {
            app.preview_scroll = app.preview_scroll.saturating_add(3);
        }
        // Shift+K: preview scroll up
        KeyCode::Char('K') => {
            app.preview_scroll = app.preview_scroll.saturating_sub(3);
        }
        // Toggle favorite on selected session
        KeyCode::Char('*') => {
            if let Some(session) = app.filtered_sessions.get(app.selected) {
                let session_id = session.session_id.clone();
                match app.db.toggle_favorite(&session_id) {
                    Ok(true) => {
                        app.favorites.insert(session_id);
                        app.set_status("Added to favorites ⭐".to_string());
                    }
                    Ok(false) => {
                        app.favorites.remove(&session_id);
                        app.set_status("Removed from favorites".to_string());
                    }
                    Err(e) => {
                        app.set_status(format!("Error: {e}"));
                    }
                }
            }
        }
        // Shift+S: open stats dashboard (already pre-loaded at startup)
        KeyCode::Char('S') => {
            app.mode = Mode::Stats;
        }
        // Shift+R: re-summarize sessions without summaries (Ollama)
        KeyCode::Char('R') => {
            app.set_status("Summarizing new sessions...".to_string());
            let db_path = crate::db::Database::default_path();
            super::spawn_summarize(&db_path, app.summarize_tx.clone());
        }
        // Shift+F: toggle favorites-only filter
        KeyCode::Char('F') => {
            app.show_favorites_only = !app.show_favorites_only;
            app.apply_filters();
        }
        // t: enter tag input mode
        KeyCode::Char('t') => {
            if app.filtered_sessions.get(app.selected).is_some() {
                app.tag_input.clear();
                app.mode = Mode::TagInput;
            }
        }
        // y: yank (copy) session ID to clipboard
        KeyCode::Char('y') => {
            if let Some(session) = app.filtered_sessions.get(app.selected) {
                let sid = session.session_id.clone();
                let cmd = format!("claude --resume {sid}");
                if let Ok(mut child) = std::process::Command::new("pbcopy")
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                {
                    if let Some(stdin) = child.stdin.as_mut() {
                        use std::io::Write;
                        let _ = stdin.write_all(cmd.as_bytes());
                    }
                    let _ = child.wait();
                    app.set_status(format!("Copied: {cmd}"));
                } else {
                    app.set_status("Clipboard not available".to_string());
                }
            }
        }
        // ?: show help
        KeyCode::Char('?') => {
            app.mode = Mode::Help;
        }
        // x / Delete: prompt to delete the selected session
        KeyCode::Char('x') | KeyCode::Delete => {
            if let Some(session) = app.filtered_sessions.get(app.selected) {
                app.pending_delete = Some((session.session_id.clone(), session.jsonl_path.clone()));
                app.mode = Mode::ConfirmDelete;
            }
        }
        _ => {
            app.pending_g = false;
        }
    }

    // Reset pending_g on any key that isn't 'g'
    if key != KeyCode::Char('g') {
        app.pending_g = false;
    }
}

fn handle_help(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
            app.mode = Mode::Normal;
        }
        _ => {}
    }
}

fn handle_stats(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('S') => {
            app.mode = Mode::Normal;
            app.stats = None;
        }
        _ => {}
    }
}

fn handle_confirm_delete(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            if let Some((session_id, jsonl_path)) = app.pending_delete.take() {
                let _ = std::fs::remove_file(&jsonl_path);
                let _ = app.db.delete_session(&session_id);
                app.favorites.remove(&session_id);
                app.set_status("Session deleted".to_string());
                app.apply_filters();
            }
            app.mode = Mode::Normal;
        }
        _ => {
            app.pending_delete = None;
            app.mode = Mode::Normal;
            app.set_status("Delete cancelled".to_string());
        }
    }
}

fn handle_tag_input(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Char(c) => {
            app.tag_input.push(c);
        }
        KeyCode::Backspace => {
            app.tag_input.pop();
        }
        KeyCode::Enter => {
            let tag = app.tag_input.trim().to_string();
            if !tag.is_empty() {
                if let Some(session) = app.filtered_sessions.get(app.selected) {
                    let session_id = session.session_id.clone();
                    match app.db.add_tag(&session_id, &tag) {
                        Ok(()) => {
                            app.set_status(format!("Tag '{tag}' added"));
                        }
                        Err(e) => {
                            app.set_status(format!("Error: {e}"));
                        }
                    }
                }
            }
            app.tag_input.clear();
            app.mode = Mode::Normal;
        }
        KeyCode::Esc => {
            app.tag_input.clear();
            app.mode = Mode::Normal;
        }
        _ => {}
    }
}

fn handle_search(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Esc => {
            app.mode = Mode::Normal;
            app.search_input.clear();
            app.apply_filters();
        }
        KeyCode::Enter => {
            app.mode = Mode::Normal;
            app.apply_filters();
        }
        KeyCode::Backspace => {
            app.search_input.pop();
            app.apply_filters();
        }
        KeyCode::Char(c) => {
            app.search_input.push(c);
            app.apply_filters();
        }
        _ => {}
    }
}

fn handle_project_filter(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Esc => {
            app.mode = Mode::Normal;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            // +1 because index 0 = "All projects" (None)
            let max = app.projects.len(); // projects.len() slots + 1 "All" = max index
            app.project_selected = (app.project_selected + 1).min(max);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.project_selected > 0 {
                app.project_selected -= 1;
            }
        }
        KeyCode::Enter => {
            if app.project_selected == 0 {
                app.project_filter = None;
            } else {
                let idx = app.project_selected - 1;
                if let Some((name, _)) = app.projects.get(idx) {
                    app.project_filter = Some(name.clone());
                }
            }
            app.mode = Mode::Normal;
            app.apply_filters();
        }
        _ => {}
    }
}
