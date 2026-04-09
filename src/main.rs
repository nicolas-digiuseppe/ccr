mod config;
mod db;
mod indexer;
mod launcher;
mod parser;
mod pricing;
mod summarizer;
mod tui;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ccr", about = "Claude Code Resume — TUI session browser")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Index or re-index sessions
    Index {
        /// Full re-index (ignore mtime cache)
        #[arg(long)]
        full: bool,
    },
    /// Search sessions non-interactively
    Search {
        /// Search query
        query: String,
    },
    /// List projects with session counts
    Projects,
    /// Generate AI summaries via Ollama (requires: brew install ollama)
    Summarize,
    /// Launch a session directly by ID
    Resume {
        /// Session UUID
        session_id: String,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Index { full }) => {
            let db_path = db::Database::default_path();
            let db = db::Database::open(&db_path)?;
            let projects_dir = dirs::home_dir().unwrap().join(".claude/projects");
            let config = config::Config::load();
            let (indexed, total) = indexer::run_index(&db, &projects_dir, full, &config.exclude_projects)?;
            println!("Indexed {indexed}/{total} sessions");
            Ok(())
        }
        Some(Commands::Search { query }) => {
            let db_path = db::Database::default_path();
            let db = db::Database::open(&db_path)?;
            let results = db.search_fulltext(&query)?;
            for s in &results {
                println!("{} | {} | {}", s.last_modified, s.project, s.first_message);
            }
            Ok(())
        }
        Some(Commands::Projects) => {
            let db_path = db::Database::default_path();
            let db = db::Database::open(&db_path)?;
            let projects = db.list_projects()?;
            for (name, count) in &projects {
                println!("{name}: {count} sessions");
            }
            Ok(())
        }
        Some(Commands::Summarize) => {
            let db_path = db::Database::default_path();
            let db = db::Database::open(&db_path)?;
            let (done, skipped, _errors) = summarizer::batch_summarize(&db);
            println!("Done: {done} summarized, {skipped} skipped");
            Ok(())
        }
        Some(Commands::Resume { session_id }) => {
            let config = config::Config::load();
            let db = db::Database::open(&db::Database::default_path())?;
            let sessions = db.list_sessions(None, None)?;
            let project_path = sessions.iter()
                .find(|s| s.session_id == session_id)
                .map(|s| s.project_path.clone())
                .unwrap_or_else(|| ".".to_string());
            let request = launcher::build_launch_request(
                &config.launch_command, &session_id, &project_path,
            );
            launcher::exec_session(&request);
        }
        None => {
            // Default: launch TUI, loop back after claude exits
            let db_path = db::Database::default_path();
            let config = config::Config::load();
            loop {
                let db = db::Database::open(&db_path)?;
                match tui::run(db, config.clone())? {
                    Some(request) => {
                        if let Err(e) = launcher::spawn_session(&request) {
                            eprintln!("{}", e);
                        }
                    }
                    None => break, // User pressed q — exit ccr
                }
            }
            Ok(())
        }
    }
}
