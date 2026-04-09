# ccr — Claude Code Resume

A terminal UI for browsing, searching, and resuming [Claude Code](https://docs.anthropic.com/en/docs/claude-code) sessions across all your projects.

Sessions are scattered across `~/.claude/projects/` as JSONL files. **ccr** indexes them into a local SQLite database with full-text search, and lets you navigate, filter, and jump back into any session instantly.

## Install

```bash
cargo install --git https://github.com/nicolas-digiuseppe/ccr.git
```

### Optional: AI summaries

ccr can generate one-line AI summaries for each session using [Ollama](https://ollama.com) (local, free):

```bash
brew install ollama
brew services start ollama
ollama pull qwen2.5:7b
ccr summarize              # batch summarize all sessions
```

Summaries are also generated automatically in the background when ccr launches.

## Usage

```bash
ccr                        # launch TUI (default)
ccr index                  # re-index sessions
ccr index --full           # full re-index (ignore mtime cache)
ccr search "query"         # search sessions (non-interactive, FTS)
ccr projects               # list projects with session counts
ccr summarize              # generate AI summaries via Ollama
ccr resume <session_id>    # resume a session directly (replaces process)
```

## Keybindings

Press `?` in the TUI for the full list.

### Navigation

| Key | Action |
|-----|--------|
| `j/k` `↑/↓` | Move up/down |
| `gg` / `G` | Jump to top / bottom |
| `Ctrl+d/u` | Page down / up |
| `Tab` | Toggle preview panel |
| `J/K` | Scroll preview |
| Mouse scroll | Scroll list |
| Mouse click | Select session |

### Actions

| Key | Action |
|-----|--------|
| `Enter` | Open session (loops back to ccr after exit) |
| `y` | Copy `claude --resume <id>` to clipboard |
| `*` | Toggle favorite |
| `F` | Show favorites only |
| `t` | Add tag |
| `x` | Delete session (with confirmation) |

### Search & Filter

| Key | Action |
|-----|--------|
| `/` | Search (fuzzy on metadata + full-text on content) |
| `p` | Filter by project |
| `d` | Cycle date filter (all / today / week / month) |
| `s` | Cycle sort (date / duration / messages / project) |
| `e` | Cycle empty filter (hide / show all / only empty) |
| `c` | Clear all filters |

### Other

| Key | Action |
|-----|--------|
| `S` | Stats dashboard (tokens, cost, activity) |
| `r` | Re-index sessions |
| `R` | Re-summarize new sessions (Ollama) |
| `?` | Help |
| `q` / `Esc` | Quit |

## How it works

1. **Index** — scans `~/.claude/projects/` for JSONL session files, parses metadata (messages, timestamps, duration), and stores in `~/.claude/ccr.db` (SQLite + FTS5)
2. **Search** — hybrid search: fuzzy matching on metadata (project, first message, summary) via [nucleo](https://github.com/helix-editor/nucleo), plus FTS5 full-text on session content
3. **Resume** — on `Enter`, ccr spawns `claude --resume <id>` as a child process, then loops back to the TUI when claude exits
4. **Summarize** — calls local Ollama (qwen2.5:7b) to generate one-line session summaries, stored in DB and preserved across re-indexes

## Config

Optional config file at `~/.claude/ccr.toml`:

```toml
launch_command = "claude --resume"   # command to resume sessions
terminal = "warp"                     # unused for now
exclude_projects = ["tmp", "test"]    # projects to skip during indexing
theme = "dark"                        # unused for now
```

## Architecture

```
src/
├── main.rs          # CLI (clap) + TUI loop
├── config.rs        # TOML config loader
├── db.rs            # SQLite schema, queries, stats, FTS5 search
├── indexer.rs       # Project scanner, JSONL parser, incremental indexing
├── parser.rs        # JSONL line/session parsing, system content filtering
├── launcher.rs      # Session launching (exec/spawn), alias resolution
├── summarizer.rs    # Ollama integration for AI summaries
└── tui/
    ├── mod.rs       # App state, event loop, filters
    ├── keybinds.rs  # Key + mouse handlers
    ├── layout.rs    # Main layout, dashboard, popups
    ├── list.rs      # Session list rendering
    ├── preview.rs   # Message preview panel
    └── search.rs    # Search bar + filter badges
```

## Dependencies

- [ratatui](https://ratatui.rs) — terminal UI framework
- [rusqlite](https://github.com/rusqlite/rusqlite) (bundled) — SQLite with FTS5
- [nucleo-matcher](https://github.com/helix-editor/nucleo) — fuzzy/substring matching
- [clap](https://github.com/clap-rs/clap) — CLI argument parsing
- [chrono](https://github.com/chronotope/chrono) — date/time handling

## Requirements

- Rust 1.70+
- macOS (uses `pbcopy` for clipboard, `exec()` for session launch)
- Claude Code installed (`claude` in PATH or alias in `~/.zshrc`)
- Ollama (optional, for AI summaries)
