# Recipio — Implementation Plan

Source brief: `PROMPT.md` (authoritative requirements). This file records the plan,
dependency choices, and decisions agreed with the owner. Update it when a decision changes.

Status of each phase lives in `CLAUDE.md` → "Current status".

---

## 1. Architecture

```
React + TS (Vite)  ──invoke()/events──►  Tauri v2 commands (thin)  ──►  Rust core modules
  UI + view state only                      src-tauri/src/commands/*      src-tauri/src/core/*
```

Rust layout (`src-tauri/src/`):

| Module                 | Responsibility                                                                                  |
| ---------------------- | ----------------------------------------------------------------------------------------------- |
| `db/`                  | Connection, migrations (`rusqlite_migration`), repositories (recipes, categories, tags, search) |
| `ingredient_parser.rs` | Heuristic ingredient-line parser (pure, unit-tested); see §3 "Paste-to-parse"                   |
| `core/fraction.rs`     | Decimal → friendly fraction formatting (pure, unit-tested)                                      |
| `core/duration.rs`     | ISO 8601 duration parsing (pure, unit-tested)                                                   |
| `core/jsonld.rs`       | schema.org Recipe extraction from HTML (pure, unit-tested with fixtures)                        |
| `media/`               | Media folder paths, photo import, custom `recipio-media://` protocol with Range support         |
| `tools/`               | yt-dlp / ffmpeg / deno download, checksum verify, chmod, quarantine strip, version query        |
| `video/`               | Download job manager (spawn, progress parse, cancel, retry, transcode fallback)                 |
| `import/`              | Web page fetch + image download                                                                 |
| `backup/`              | Zip export/import                                                                               |
| `commands/`            | Tauri command wrappers; map internal errors → user-facing `AppError`                            |
| `error.rs`             | `thiserror` error enum, serialized to `{ kind, message }` for the UI                            |

Shared types: Rust structs derive `ts_rs::TS`, exported to `src/bindings/` so the
frontend types never drift from the backend.

Concurrency: one `rusqlite::Connection` behind a `Mutex` in Tauri state (single user,
local app — no pool needed). Long-running work (downloads, imports, backup) runs in
`tauri::async_runtime` tasks and reports via Tauri events.

### App data layout (via Tauri `app_data_dir()`)

```
<app_data>/
  recipio.sqlite3
  logs/recipio.log
  media/
    photos/<recipe_uuid>.<ext>
    videos/<recipe_uuid>.mp4
    tmp/                      # partial downloads, cleaned on startup
  bin/
    yt-dlp[.exe]  ffmpeg[.exe]  ffprobe[.exe]  deno[.exe]
    tools.json                # installed versions + sha256
```

## 2. Data model (SQLite)

- **IDs: UUID v4 (TEXT)**, generated when the editor opens a new recipe. Reason: a video
  download can start before the recipe is first saved, and the file must be named after
  the recipe id. Orphaned media (recipe never saved) is swept on startup.
- `recipes`: id, title, description, servings, prep_minutes, cook_minutes, photo_path,
  notes, rating (1–5 NULL), youtube_url, video_path, video_status
  (`none|downloading|ready|failed|missing`), source_url, calories, protein_g, carbs_g,
  fat_g, created_at, updated_at (UTC ISO 8601 text).
- `ingredients`: id, recipe_id FK CASCADE, position, is_heading (heading text stored in `item`),
  quantity REAL NULL, **quantity_max REAL NULL** (for ranges, see Q1), unit, item, prep_note.
- `steps`: id, recipe_id FK CASCADE, position, text.
- `categories` (name UNIQUE COLLATE NOCASE), `tags` (same), join tables
  `recipe_categories`, `recipe_tags`.
- `recipes_fts` FTS5 (recipe_id UNINDEXED, title, description, ingredients, steps, notes),
  maintained by Rust inside the same transaction as every recipe write
  (simpler and more testable than triggers spanning three tables).
- Migrations: `src-tauri/migrations/NNNN_name.sql`, embedded with `include_str!`,
  applied by `rusqlite_migration` at startup (uses `PRAGMA user_version`).

## 3. Key technical decisions

### Video playback / range requests

Register a custom async URI scheme `recipio-media://` that serves only files under
`<app_data>/media`, with canonicalized-path checks (no `..`, no symlink escape),
correct `Content-Type`, and full `Range: bytes=` support (206 / 416). One code path on
all three platforms, so seeking behaves identically. (Tauri's built-in asset protocol
does handle ranges in v2, but historically has had per-platform quirks; see Q3.)

### yt-dlp invocation

Argument arrays only (`std::process::Command` / `tokio::process`), never a shell.
Pass `--` before the URL. Validate URL host is youtube.com / youtu.be / m.youtube.com.

1. Metadata: `yt-dlp -J --no-playlist -- <url>` → title, thumbnail, duration.
2. Download (H.264/AAC preferred):
   `-f "bv*[height<=1080][vcodec^=avc1]+ba[acodec^=mp4a]/b[height<=1080][vcodec^=avc1][acodec^=mp4a]"`
   `--merge-output-format mp4 --ffmpeg-location <bin> --no-playlist --newline`
   `--progress-template` (JSON) → parsed and emitted as `video://progress` events.
3. Fallback if no avc1/mp4a: `bv*[height<=1080]+ba/b[height<=1080]`, then ffmpeg
   transcode `-c:v libx264 -preset veryfast -crf 20 -c:a aac -movflags +faststart`.
   UI is told "transcoding, this may take longer" with ffmpeg `-progress` output.

- Download into `media/tmp/`, atomic rename to `media/videos/<id>.mp4` on success.
- Cancel = kill child process + delete temp files. Retry = new job.
- Disk-space pre-check (`fs4::available_space`) against estimated filesize from metadata.
- Error classification from yt-dlp stderr: invalid URL, private/unavailable, network,
  disk full, unknown (full text to log).

### Binary sources (verified by SHA-256 before first use)

| Tool             | Platform                    | Source                                                                                                   | Checksum source                        |
| ---------------- | --------------------------- | -------------------------------------------------------------------------------------------------------- | -------------------------------------- |
| yt-dlp           | all                         | github.com/yt-dlp/yt-dlp releases (`yt-dlp_macos`, `yt-dlp.exe`, `yt-dlp_linux`, `yt-dlp_linux_aarch64`) | `SHA2-256SUMS` in the same release     |
| ffmpeg + ffprobe | Windows, Linux (x64, arm64) | github.com/yt-dlp/FFmpeg-Builds (BtbN-based, patched for yt-dlp, GPL static)                             | `checksums.sha256` in the same release |
| ffmpeg + ffprobe | macOS (arm64, x86_64)       | ffmpeg.martin-riedl.de (static, signed macOS builds, both archs)                                         | per-file `.sha256` published alongside |
| deno             | all                         | github.com/denoland/deno releases                                                                        | per-asset `.sha256sum`                 |

Trust model: checksums are fetched over HTTPS from the same publisher (protects against
corruption/truncation/mirror tampering, not a compromised publisher). See Q4.
macOS: `chmod 755`, remove `com.apple.quarantine` xattr defensively.

### Paste-to-parse ingredients

Goal (brief, Phase 3): paste a block of ingredient text, get structured lines, review them
before they are applied. The same parser will turn JSON-LD `recipeIngredient` strings into
lines in Phase 5.

- **Rust** `ingredient_parser.rs`: `parse_block(text)` → one `ParsedIngredient` per
  non-blank line; `parse_line(line)` is the reusable unit. Command `parse_ingredients`.
  Pure and unit-tested. It never errors — what it can't pick apart goes into `item`.
- **Output shape** = an editor row (`is_heading`, `quantity`, `unit`, `item`, `prep_note`,
  all strings). `quantity` is Rust-formatted display text ("1½", "2–3") that
  `quantity::parse` accepts back, so pasted lines go through the normal save path.
- **Per line, in order:**
  1. Strip list markers (`•`, `*`, `▢`, `- `, …) and collapse whitespace.
  2. Heading if it ends with `:`, starts with `#`, is all caps without digits ("SAUCE"),
     or is a short "For the …" phrase without a comma. Trailing `:` removed.
  3. Split a number glued to a word ("200g", "1½cups") — only a plain number prefix,
     so "4-inch" stays whole.
  4. Quantity = the longest run of up to 4 leading tokens that `quantity::parse` accepts
     ("1 1/2", "2 to 3", "1-1/2"). "a"/"an" counts as 1 only when a unit follows.
  5. Unit = a known unit word right after the quantity (fixed list: volume, weight, and
     counts like clove/can/pinch/stick; `fl oz`; trailing `.` dropped; case kept as typed),
     optionally preceded by a size in parentheses: "1 (14 oz) can". Without a quantity a
     unit counts only before "of" ("Pinch of salt"). Size words (large, small) stay in the item.
  6. Drop a leading "of". Prep note = text after the first comma, else a trailing
     "(…)" ("onion (finely chopped)").
  7. If nothing is left for the item ("2 cloves"), the unit word becomes the item.
- **UI:** "Paste ingredients" button under the ingredient rows, and pasting multi-line
  text into any ingredient field, both open a dialog: textarea on the left, live parsed
  preview (Rust, per keystroke) on the right, "Add N ingredients". Lines are inserted
  after the current row (replacing it if blank) and stay editable in the normal editor —
  the preview is the review step; there is no per-line editing inside the dialog.
- Not attempted: unit conversion, ingredient name normalisation, multi-line ingredients,
  numbered lists ("1. flour" reads as quantity 1).

### Other

- Screen wake in cooking mode: `keepawake` crate (IOKit / SetThreadExecutionState /
  D-Bus inhibit) — works on all platforms, unlike the Web Wake Lock API in WebKitGTK.
- Logging: `tauri-plugin-log` with a file target redirected to `<app_data>/logs/`.
- External links: `tauri-plugin-opener` (system browser).
- File pickers: `tauri-plugin-dialog`.
- No network calls except: YouTube (via yt-dlp), tool downloads, recipe page import,
  recipe image download.

## 4. Dependencies

### Rust crates

| Crate                                                            | Why                                                      |
| ---------------------------------------------------------------- | -------------------------------------------------------- |
| `tauri` 2, `tauri-build`                                         | Framework                                                |
| `tauri-plugin-dialog`, `tauri-plugin-opener`, `tauri-plugin-log` | Official plugins: file pickers, open URLs, log file      |
| `rusqlite` (`bundled`)                                           | SQLite incl. FTS5, no system dependency; simple sync API |
| `rusqlite_migration`                                             | Versioned migrations via `user_version`                  |
| `serde`, `serde_json`                                            | Serialization                                            |
| `thiserror`                                                      | Error types                                              |
| `uuid` (v4)                                                      | Recipe IDs                                               |
| `chrono`                                                         | Timestamps                                               |
| `reqwest` (`rustls-tls`, `stream`, no default features)          | HTTP for tools, pages, images; no OpenSSL dependency     |
| `sha2`, `hex`                                                    | Checksum verification                                    |
| `zip`                                                            | Backup archives; Windows ffmpeg archive                  |
| `tar`, `xz2`                                                     | Linux ffmpeg `.tar.xz` archives                          |
| `scraper`                                                        | Locate `<script type="application/ld+json">` blocks      |
| `fs4`                                                            | Free disk space check                                    |
| `keepawake`                                                      | Keep screen awake in cooking mode                        |
| `ts-rs`                                                          | Generate TS types from Rust structs                      |
| `tokio` (via Tauri runtime; `process`, `io-util`)                | Child process streaming                                  |
| `log`                                                            | Logging facade                                           |
| dev: `tempfile`, `pretty_assertions`                             | Tests                                                    |

Hand-written (no crate): ingredient parser, fraction formatting, scaling, ISO 8601
durations, range-request handling.

### npm packages

| Package                                                                              | Why                                                                    |
| ------------------------------------------------------------------------------------ | ---------------------------------------------------------------------- |
| `react`, `react-dom`, `typescript`, `vite`, `@vitejs/plugin-react`                   | Stack                                                                  |
| `@tauri-apps/api`, `@tauri-apps/cli`, `@tauri-apps/plugin-dialog`, `-opener`, `-log` | Tauri bindings                                                         |
| `react-router`                                                                       | Routes: library, recipe view, editor, cooking mode, settings           |
| `@tanstack/react-query`                                                              | Cache + invalidation over `invoke()` calls; avoids a hand-rolled store |
| `@dnd-kit/core`, `@dnd-kit/sortable`                                                 | Accessible, keyboard-capable drag-to-reorder                           |
| `lucide-react`                                                                       | Icons                                                                  |
| `eslint`, `typescript-eslint`, `eslint-plugin-react-hooks`, `prettier`               | Lint/format                                                            |

Styling: plain CSS with CSS custom properties (light/dark via `prefers-color-scheme`),
CSS modules. No UI framework. Type-ahead pickers are a small custom combobox.

## 5. Phases (from brief)

1. Foundation 2. Organize 3. Ingredients plus 4. Video 5. Web import
2. Cooking mode + backup 7. Packaging (GitHub Actions, README)

Each phase ends with: clippy + fmt + eslint + prettier clean, tests passing, a git commit,
test instructions for the owner, and a stop for approval.

## 6. Open questions / decisions log

| #   | Question                                                                            | Decision                                                                                                       |
| --- | ----------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| Q1  | Ranges ("2-3 eggs"): add nullable `quantity_max` to ingredients?                    | **Yes** — add `quantity_max`                                                                                   |
| Q2  | YouTube now requires a JS runtime for yt-dlp (Deno). Also manage Deno?              | **Yes** — download Deno from denoland/deno GitHub releases, verify `.sha256sum`                                |
| Q3  | Use custom range-capable `recipio-media://` protocol from the start?                | **Yes** — custom protocol from the start                                                                       |
| Q4  | Checksum trust: publisher manifests over HTTPS acceptable (vs pinned hashes / GPG)? | **Yes** — publisher checksums are sufficient for v1                                                            |
| Q5  | Backup import = full replace of library (with warning), not merge?                  | **Yes** — full replace after warning                                                                           |
| Q6  | Unsigned/un-notarized installers acceptable for v1?                                 | **Yes** — unsigned is fine                                                                                     |
| Q7  | Bundle identifier (determines app data path), e.g. `io.github.<user>.recipio`?      | **`io.github.bvdon.recipio`** (owner deferred to Claude)                                                       |
| Q8  | GitHub repo: create/push at Phase 1 or Phase 7? Repo name/owner?                    | Public repo `bvdon/recipio` (timing: see CLAUDE.md status)                                                     |
| Q9  | Testing scope                                                                       | macOS-only testing is fine for now; Windows/Linux via CI later                                                 |
| Q10 | Servings scaling (brief, Phase 3)                                                   | **Dropped** by the owner (2026-10-22) — no servings control, no `scaling.rs`                                   |
| Q11 | Quantity entry                                                                      | Typed as text (`3/4`, `1 1/2`, `½`, `2-3`), parsed in Rust (`quantity.rs`), shown as fractions (`¾`, `1½`)     |
| Q12 | Paste-to-parse review                                                               | Read-only preview in a dialog, then lines are added to the editor for any fixes (no editing inside the dialog) |
| Q13 | App icon                                                                            | The logo's chef hat (lucide `ChefHat`), white on the accent terracotta; source `src-tauri/icons/icon.svg`      |
