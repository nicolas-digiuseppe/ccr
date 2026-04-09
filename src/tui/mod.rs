pub mod keybinds;
pub mod layout;
pub mod list;
pub mod preview;
pub mod search;

use anyhow::Result;
use chrono::{DateTime, Utc, Duration};
use ratatui::crossterm::event::{self, Event, KeyEventKind, MouseEventKind, EnableMouseCapture, DisableMouseCapture};
use std::io::BufRead;

use crate::config::Config;
use crate::db::{Database, SessionInfo};
use crate::parser;

// ─── Mode enums ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Normal,
    Search,
    ProjectFilter,
    TagInput,
    ConfirmDelete,
    Stats,
    Help,
}


#[derive(Debug, Clone, PartialEq)]
pub enum DateFilter {
    All,
    Today,
    ThisWeek,
    ThisMonth,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SortMode {
    Date,       // last_modified DESC (default)
    Duration,   // duration_minutes DESC
    Messages,   // message_count DESC
    Project,    // project ASC, then date DESC
}

impl SortMode {
    pub fn next(&self) -> SortMode {
        match self {
            SortMode::Date => SortMode::Duration,
            SortMode::Duration => SortMode::Messages,
            SortMode::Messages => SortMode::Project,
            SortMode::Project => SortMode::Date,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            SortMode::Date => "Date",
            SortMode::Duration => "Duration",
            SortMode::Messages => "Messages",
            SortMode::Project => "Project",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EmptyFilter {
    HideEmpty,   // default — hide 0-message sessions
    ShowAll,     // show everything
    OnlyEmpty,   // show only 0-message sessions (for cleanup)
}

impl EmptyFilter {
    pub fn next(&self) -> EmptyFilter {
        match self {
            EmptyFilter::HideEmpty => EmptyFilter::ShowAll,
            EmptyFilter::ShowAll => EmptyFilter::OnlyEmpty,
            EmptyFilter::OnlyEmpty => EmptyFilter::HideEmpty,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            EmptyFilter::HideEmpty => "",
            EmptyFilter::ShowAll => "[all]",
            EmptyFilter::OnlyEmpty => "[empty only]",
        }
    }
}

impl DateFilter {
    pub fn next(&self) -> DateFilter {
        match self {
            DateFilter::All => DateFilter::Today,
            DateFilter::Today => DateFilter::ThisWeek,
            DateFilter::ThisWeek => DateFilter::ThisMonth,
            DateFilter::ThisMonth => DateFilter::All,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            DateFilter::All => "All",
            DateFilter::Today => "Today",
            DateFilter::ThisWeek => "ThisWeek",
            DateFilter::ThisMonth => "ThisMonth",
        }
    }
}

// ─── App struct ───────────────────────────────────────────────────────────────

pub struct App {
    #[allow(dead_code)]
    pub sessions: Vec<SessionInfo>,
    pub filtered_sessions: Vec<SessionInfo>,
    pub selected: usize,
    pub mode: Mode,

    pub search_input: String,
    pub project_filter: Option<String>,
    pub date_filter: DateFilter,
    pub sort_mode: SortMode,
    pub projects: Vec<(String, u32)>,
    pub project_selected: usize,
    pub preview_messages: Vec<(String, String)>,
    pub preview_scroll: u16,
    pub show_preview: bool,
    pub status_message: Option<String>,
    pub status_ttl: u8,  // ticks remaining before clearing status_message
    pub db: Database,
    pub config: Config,
    pub should_quit: bool,
    pub launch: Option<crate::launcher::LaunchRequest>,
    pub show_favorites_only: bool,
    pub favorites: std::collections::HashSet<String>,
    pub tag_input: String,
    pub pending_delete: Option<(String, String)>,  // (session_id, jsonl_path)
    pub stats: Option<crate::db::Stats>,
    pub empty_filter: EmptyFilter,
    pub pending_g: bool,
    pub visible_height: u16,
    pub summarize_tx: std::sync::mpsc::Sender<(String, String)>,  // (session_id, summary)
    pub summarize_rx: std::sync::mpsc::Receiver<(String, String)>,
}

impl App {
    pub fn new(db: Database, config: Config) -> Result<Self> {
        let sessions = db.list_sessions(None, None)?;
        let projects_raw = db.list_projects()?;
        let projects: Vec<(String, u32)> = projects_raw
            .into_iter()
            .map(|(name, count)| (name, count as u32))
            .collect();

        // Load favorites from db into a HashSet for fast lookup
        let fav_ids = db.list_favorites().unwrap_or_default();
        let favorites: std::collections::HashSet<String> = fav_ids.into_iter().collect();

        let (sum_tx, sum_rx) = std::sync::mpsc::channel();

        let filtered_sessions = sessions
            .iter()
            .map(|s| SessionInfo {
                session_id: s.session_id.clone(),
                project: s.project.clone(),
                project_path: s.project_path.clone(),
                jsonl_path: s.jsonl_path.clone(),
                first_message: s.first_message.clone(),
                slug: s.slug.clone(),
                started_at: s.started_at.clone(),
                last_modified: s.last_modified.clone(),
                message_count: s.message_count,
                file_size: s.file_size,
                duration_minutes: s.duration_minutes,
                summary: s.summary.clone(),
            })
            .collect();

        Ok(App {
            sessions,
            filtered_sessions,
            selected: 0,
            mode: Mode::Normal,

            search_input: String::new(),
            project_filter: None,
            date_filter: DateFilter::All,
            sort_mode: SortMode::Date,
            projects,
            project_selected: 0,
            preview_messages: Vec::new(),
            preview_scroll: 0,
            show_preview: true,
            status_message: None,
            status_ttl: 0,
            db,
            config,
            should_quit: false,
            launch: None,
            show_favorites_only: false,
            favorites,
            tag_input: String::new(),
            pending_delete: None,
            stats: None,
            empty_filter: EmptyFilter::HideEmpty,
            pending_g: false,
            visible_height: 20,
            summarize_tx: sum_tx,
            summarize_rx: sum_rx,
        })
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some(msg.into());
        self.status_ttl = 20; // ~2 seconds at 100ms poll
    }

    pub fn tick_status(&mut self) {
        if self.status_ttl > 0 {
            self.status_ttl -= 1;
            if self.status_ttl == 0 {
                self.status_message = None;
            }
        }
    }

    pub fn load_preview(&mut self) {
        self.preview_messages.clear();
        self.preview_scroll = 0;

        let session = match self.filtered_sessions.get(self.selected) {
            Some(s) => s,
            None => return,
        };

        let path = std::path::Path::new(&session.jsonl_path);
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(_) => return,
        };

        let reader = std::io::BufReader::new(file);
        let mut all: Vec<(String, String)> = Vec::new();

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };

            if let Some(msg) = parser::parse_jsonl_line(&line) {
                // Skip injected skill prompts and generic assistant responses
                match msg.msg_type {
                    parser::MsgType::User if parser::is_injected_content(&msg.text) => continue,
                    parser::MsgType::Assistant if parser::is_generic_assistant(&msg.text) => continue,
                    _ => {}
                }
                let role = match msg.msg_type {
                    parser::MsgType::User => "user".to_string(),
                    parser::MsgType::Assistant => "assistant".to_string(),
                };
                let text: String = msg.text.chars().take(500).collect();
                all.push((role, text));
            }
        }

        const HEAD: usize = 5;

        if all.len() <= HEAD + 1 {
            // Few enough messages — show all
            self.preview_messages = all;
        } else {
            // First HEAD messages + separator + last message
            for msg in all.iter().take(HEAD) {
                self.preview_messages.push(msg.clone());
            }
            let skipped = all.len() - HEAD - 1;
            self.preview_messages.push((
                "separator".to_string(),
                format!("··· {} more messages ···", skipped),
            ));
            self.preview_messages.push(all.last().unwrap().clone());
        }
    }

    pub fn apply_filters(&mut self) {
        let all_sessions = self.db
            .list_sessions(self.project_filter.as_deref(), None)
            .unwrap_or_default();

        let mut sessions = all_sessions;

        // Empty session filter
        match self.empty_filter {
            EmptyFilter::HideEmpty => sessions.retain(|s| s.message_count > 0),
            EmptyFilter::OnlyEmpty => sessions.retain(|s| s.message_count == 0),
            EmptyFilter::ShowAll => {}
        }

        // Apply favorites-only filter
        if self.show_favorites_only {
            sessions.retain(|s| self.favorites.contains(&s.session_id));
        }

        // Apply date filter in memory
        if self.date_filter != DateFilter::All {
            let now: DateTime<Utc> = Utc::now();
            sessions.retain(|s| {
                let session_time = s.last_modified.parse::<i64>()
                    .ok()
                    .and_then(|ts| DateTime::from_timestamp(ts, 0))
                    .or_else(|| {
                        DateTime::parse_from_rfc3339(&s.last_modified)
                            .ok()
                            .map(|dt| dt.with_timezone(&Utc))
                    });

                match session_time {
                    None => true,
                    Some(t) => {
                        match self.date_filter {
                            DateFilter::All => true,
                            DateFilter::Today => {
                                now.signed_duration_since(t) < Duration::hours(24)
                            }
                            DateFilter::ThisWeek => {
                                now.signed_duration_since(t) < Duration::days(7)
                            }
                            DateFilter::ThisMonth => {
                                now.signed_duration_since(t) < Duration::days(30)
                            }
                        }
                    }
                }
            });
        }

        // Hybrid search: metadata fuzzy + FTS5 content
        if !self.search_input.is_empty() {
            use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
            use std::collections::HashSet;

            // Phase 1: nucleo fuzzy on metadata fields
            let pattern = Pattern::new(
                &self.search_input,
                CaseMatching::Ignore,
                Normalization::Smart,
                AtomKind::Fuzzy,
            );
            let mut matcher = nucleo_matcher::Matcher::new(nucleo_matcher::Config::DEFAULT);
            let mut buf = Vec::new();

            let mut meta_scored: Vec<(SessionInfo, u32)> = sessions
                .into_iter()
                .filter_map(|s| {
                    let hay_msg = nucleo_matcher::Utf32Str::new(&s.first_message, &mut buf);
                    let s1 = pattern.score(hay_msg, &mut matcher);
                    buf.clear();
                    let hay_proj = nucleo_matcher::Utf32Str::new(&s.project, &mut buf);
                    let s2 = pattern.score(hay_proj, &mut matcher);
                    buf.clear();
                    let s3 = s.summary.as_deref().and_then(|sum| {
                        let hay = nucleo_matcher::Utf32Str::new(sum, &mut buf);
                        let sc = pattern.score(hay, &mut matcher);
                        buf.clear();
                        sc
                    });
                    let best = s1.max(s2).max(s3);
                    best.map(|sc| (s, sc))
                })
                .collect();
            meta_scored.sort_by(|a, b| b.1.cmp(&a.1));

            let meta_ids: HashSet<String> = meta_scored.iter().map(|(s, _)| s.session_id.clone()).collect();

            // Phase 2: FTS5 prefix search on content
            let fts_results = self.db.search_fulltext(&self.search_input).unwrap_or_default();

            // Filter FTS results: remove dupes, apply same filters
            let fts_extra: Vec<SessionInfo> = fts_results
                .into_iter()
                .filter(|s| !meta_ids.contains(&s.session_id))
                .filter(|s| {
                    if let Some(ref proj) = self.project_filter {
                        &s.project == proj
                    } else {
                        true
                    }
                })
                .filter(|s| match self.empty_filter {
                    EmptyFilter::HideEmpty => s.message_count > 0,
                    EmptyFilter::OnlyEmpty => s.message_count == 0,
                    EmptyFilter::ShowAll => true,
                })
                .filter(|s| !self.show_favorites_only || self.favorites.contains(&s.session_id))
                .collect();

            // Merge: metadata first, then content-only
            sessions = meta_scored.into_iter().map(|(s, _)| s).collect();
            sessions.extend(fts_extra);
        }

        // Apply sort (Date during search = keep relevance order)
        match self.sort_mode {
            SortMode::Date => {
                if self.search_input.is_empty() {
                    // already sorted by last_modified DESC from DB
                }
            }
            SortMode::Duration => sessions.sort_by(|a, b| b.duration_minutes.cmp(&a.duration_minutes)),
            SortMode::Messages => sessions.sort_by(|a, b| b.message_count.cmp(&a.message_count)),
            SortMode::Project => sessions.sort_by(|a, b| {
                a.project.cmp(&b.project).then(b.last_modified.cmp(&a.last_modified))
            }),
        }

        self.filtered_sessions = sessions;

        // Clamp selection
        if self.filtered_sessions.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered_sessions.len() {
            self.selected = self.filtered_sessions.len() - 1;
        }

        self.load_preview();
    }
}

/// Spawn background summarization for sessions without summaries.
pub fn spawn_summarize(db_path: &std::path::Path, tx: std::sync::mpsc::Sender<(String, String)>) {
    let db_path = db_path.to_path_buf();
    std::thread::spawn(move || {
        let db = match crate::db::Database::open(&db_path) {
            Ok(d) => d,
            Err(_) => return,
        };
        let sessions = db.sessions_needing_summary().unwrap_or_default();
        for (session_id, jsonl_path) in &sessions {
            if let Some(summary) = crate::summarizer::summarize_session(jsonl_path) {
                let _ = db.set_summary(session_id, &summary);
                let _ = tx.send((session_id.clone(), summary));
            }
        }
    });
}

// ─── Event loop ───────────────────────────────────────────────────────────────

pub fn run(db: Database, config: Config) -> Result<Option<crate::launcher::LaunchRequest>> {
    // Incremental re-index at startup (fast — skips unchanged files)
    let home = std::env::var("HOME").unwrap_or_default();
    let projects_dir = std::path::PathBuf::from(&home).join(".claude/projects");
    let _ = crate::indexer::run_index(&db, &projects_dir, false, &config.exclude_projects);

    let mut terminal = ratatui::init();

    // Enable mouse capture
    ratatui::crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;

    let mut app = App::new(db, config)?;
    app.load_preview();

    // Pre-load stats in background so dashboard is instant when opened
    app.stats = app.db.get_stats().ok();

    // Background summarization of new sessions (Ollama)
    spawn_summarize(&crate::db::Database::default_path(), app.summarize_tx.clone());

    let mut frame_area = ratatui::layout::Rect::default();
    loop {
        terminal.draw(|frame| {
            frame_area = frame.area();
            layout::render(frame, &mut app);
        })?;

        // Track visible height for page nav
        app.visible_height = frame_area.height.saturating_sub(6); // approx list area

        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    keybinds::handle_key_with_modifiers(&mut app, key.code, key.modifiers);
                }
                Event::Mouse(mouse) => {
                    if app.mode == Mode::Normal {
                        match mouse.kind {
                            MouseEventKind::ScrollDown => {
                                if !app.filtered_sessions.is_empty() {
                                    app.selected = (app.selected + 3).min(app.filtered_sessions.len() - 1);
                                    app.load_preview();
                                }
                            }
                            MouseEventKind::ScrollUp => {
                                app.selected = app.selected.saturating_sub(3);
                                app.load_preview();
                            }
                            MouseEventKind::Down(_) => {
                                // Click in list area: y offset relative to list
                                // List starts at y=4 (search bar) + 1 (border) = 5
                                let list_start_y = 5u16;
                                if mouse.row >= list_start_y {
                                    let row_in_list = (mouse.row - list_start_y) as usize;
                                    // Each item is 2 lines tall
                                    let clicked_idx = row_in_list / 2;
                                    if clicked_idx < app.filtered_sessions.len() {
                                        app.selected = clicked_idx;
                                        app.load_preview();
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        app.tick_status();

        // Check for background summary results
        while let Ok((session_id, summary)) = app.summarize_rx.try_recv() {
            for s in &mut app.filtered_sessions {
                if s.session_id == session_id {
                    s.summary = Some(summary.clone());
                }
            }
        }

        // (ccusage background channel removed — stats now loaded from DB directly)

        if app.should_quit {
            break;
        }
    }

    ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture)?;
    ratatui::restore();
    Ok(app.launch.take())
}
