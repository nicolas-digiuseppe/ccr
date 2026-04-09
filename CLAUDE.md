# CLAUDE.md

## Project

ccr (Claude Code Resume) — Rust TUI for browsing and resuming Claude Code sessions.

## Build & Test

```bash
cargo build              # debug build
cargo build --release    # release build
cargo test               # run all tests (45 tests, ~0.02s)
cargo install --path .   # install to ~/.cargo/bin/ccr
```

No environment variables required for building. Tests use in-memory SQLite and tempfiles — no external dependencies.

## Architecture

Single-binary Rust CLI. Entry point: `src/main.rs` (clap CLI → TUI event loop).

Key modules:
- `db.rs` — SQLite schema with FTS5, session CRUD, stats aggregation, schema versioning
- `parser.rs` — JSONL parsing with system/injected content filtering (`is_system_content`, `is_injected_content`, `is_generic_assistant`)
- `indexer.rs` — scans `~/.claude/projects/`, decodes encoded directory names via backtracking, incremental indexing by mtime
- `tui/mod.rs` — App state struct, `apply_filters()` is the central filter/sort pipeline, event loop with mouse support
- `tui/keybinds.rs` — all key handlers by Mode (Normal, Search, ProjectFilter, TagInput, ConfirmDelete, Stats, Help)
- `summarizer.rs` — calls local Ollama HTTP API (localhost:11434) for AI session summaries

## Conventions

- ANSI 16 colors only (no RGB) — respects terminal theme
- All DB mutations go through `db.rs` methods
- Parser filters: `is_injected_content()` gates what becomes `first_message`, `is_generic_assistant()` gates assistant fallbacks
- AI summaries (Ollama) are stored in the `summary` column and preserved across re-indexes via `ON CONFLICT DO UPDATE ... CASE`
- Background tasks use `mpsc::channel` for AI summarization
- Search is hybrid: nucleo fuzzy on metadata (project, first_message, summary) + FTS5 prefix queries on content, merged with dedup
- Status messages auto-clear after ~2s via TTL counter

## Data

- DB: `~/.claude/ccr.db` (SQLite WAL mode)
- Config: `~/.claude/ccr.toml`
- Sessions: `~/.claude/projects/<encoded-path>/*.jsonl`

## Gotchas

- `decode_project_dir()` in indexer.rs uses filesystem validation — tests must use paths that exist (e.g., `/tmp`)
- `INSERT OR REPLACE` would destroy AI summaries — upsert uses `ON CONFLICT DO UPDATE` with `CASE` to preserve them
- `Ctrl+d` must match before plain `d` in keybind handler (guard with `modifiers.contains(KeyModifiers::CONTROL)`)
- The `gg` keybind uses a `pending_g` bool that resets on any other key press
