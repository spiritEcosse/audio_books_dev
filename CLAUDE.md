# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A Tauri 2 desktop app (Rust backend + React 19/TypeScript/Vite frontend, pnpm) for turning stories into illustrated, narrated audiobook videos. The user chats with Claude about one story; Claude edits the story text and splits it into scenes (narrated text + English image prompt). UI strings and many prompts are in Russian. The README is the unmodified Tauri template.

## Commands

- `pnpm install`
- `pnpm tauri dev` — run the full app (Vite dev server + Rust backend). `pnpm dev` alone runs only the web frontend, where `invoke` calls will fail.
- `pnpm build` — `tsc && vite build` (type-check + frontend bundle). This is the only frontend check; there is no linter or test runner configured.
- `pnpm tauri build` — production bundle.
- Rust: `cd src-tauri && cargo check` / `cargo build`. There are no tests yet; `cargo test <name>` runs a single one once `#[test]`s exist.

`.env` (see `.env.example`) holds `ELEVENLABS_API_KEY` and `ANTHROPIC_API_KEY`. Optional `CLAUDE_BIN` env var overrides the `claude` binary path.

## Architecture

Frontend (`src/`) is thin: `api.ts` holds the TypeScript types and typed wrappers around Tauri `invoke`; components just render state. All logic lives in Rust commands registered in `src-tauri/src/lib.rs`, which also holds `AppState` (a `Mutex<Connection>` plus the workspace dir).

Design points that span several Rust modules:

- **Claude is driven through the `claude` CLI, not the API** (`claude.rs`). It is spawned headless (`-p --output-format json`) with tools, user settings, MCP servers and slash commands disabled (`HEADLESS_ARGS`) to avoid loading huge unrelated context. The binary is found via a login shell because a Finder-launched app has a minimal PATH. Only model ids in `claude::MODELS` ever reach `--model`; the choice is persisted in the `settings` table. Chat continuity uses the CLI session id stored in `chat_sessions`.
- **JSON envelope protocol** (`chat.rs`): the system prompt forces Claude to reply with one JSON object `{reply, scenes, story_text}`. `parse_envelope` tolerates plain-text replies and never lets an empty scene list wipe scenes. System prompt = base protocol + `prompts/audience.md` + the story's writing profile body.
- **Writing profiles** (`profiles.rs`, `prompts/profiles/*.md`): built-ins (`fairy_tale`, `novel`) are embedded with `include_str!`; users can add custom `.md` files (id prefix `custom:`, size-capped) to `<workspace>/profiles`. Per-story `story_settings` (profile, audience age group, target minutes) feed the prompt; `stats.rs` computes word/minute stats. `prompts/NOTICE.md` records the licensed sources the profiles were adapted from and is not sent to Claude.
- **Revisions and confirmation** (`actions.rs`, `db.rs`): state-changing operations snapshot the previous state into `revisions` (kinds `story_text`, `scenes`) so they can be rolled back. Claude's `story_text` is stored as a pending `proposal` on a message and applied only when the user accepts it; the outcome is queued in `pending_notes` and prepended to the next user message as `<app_notes>` so Claude knows what happened outside the chat.
- **Storage** (`db.rs`): SQLite via `rusqlite` (bundled), schema as plain `CREATE TABLE IF NOT EXISTS` statements. There is no migration framework, so changes to existing tables need manual `ALTER` handling.
- Errors are a single `AppError` (`error.rs`) serialized to the frontend.
