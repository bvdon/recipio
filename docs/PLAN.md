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

### yt-dlp invocation (as built, `video.rs`)

Argument arrays only (`tokio::process`, `process.rs`), never a shell; `--` before the URL.
Every call: `--ignore-config --no-playlist --js-runtimes deno:<bin>/deno
--ffmpeg-location <bin> --cache-dir <bin>/cache --encoding utf-8`.
URL check (`video::youtube_url`): http(s), host youtube.com / www. / m. / music. /
youtu.be / youtube-nocookie, must name a video (`v=`, `/shorts/`, `/embed/`, `/live/`,
youtu.be path), no credentials.

1. Metadata + format choice: `yt-dlp -J -f <FORMAT> -S res:1080 -- <url>` with
   `FORMAT = bv*[vcodec^=avc1]+ba[acodec^=mp4a]/b[vcodec^=avc1][acodec^=mp4a]/bv*+ba/b`.
   `-S res:1080` = largest resolution up to 1080p measured on the **short side**, so
   vertical videos (1080×1920) aren't pushed down to 480p (a plain `[height<=1080]`
   filter would). Rust then rejects a choice whose short side is > 1080.
   Title, duration and thumbnail come from the same JSON; the thumbnail (https,
   ytimg/ggpht hosts only, ≤ 10 MB, jpeg/webp/png) is staged like a chosen photo.
2. Download exactly the chosen `format_id`: `-o tmp/<id>.dl.%(ext)s`,
   `--merge-output-format mp4` (or `mkv` when a transcode follows),
   `--progress-template "download:RECIPIO_PROGRESS %(progress)j"` and
   `--print "after_move:RECIPIO_FILE %(filepath)s"` (+ `--no-simulate --progress --newline`).
   Overall % weights video/audio parts by their sizes from step 1.
3. Only if the choice isn't avc1 + mp4a: ffmpeg `-c:v libx264 -preset veryfast -crf 20
-pix_fmt yuv420p -c:a aac -b:a 160k -movflags +faststart -progress pipe:1`; the UI
   says it may take a while.

- Download into `media/tmp/`, rename to `media/videos/<id>.mp4` on success (replacing an
  older video). The DB row is updated then (`video_path`, `ready`, `youtube_url`,
  `updated_at`); if the recipe isn't saved yet, `save_recipe` links the file later.
- One job per recipe, state in memory (`Videos`), pushed as `video://job` events
  (throttled to 4/s) and readable with `get_video_job` when a page opens mid-download.
- Cancel kills the whole process tree (own process group / `taskkill /T`); all tools are
  also killed on app exit. Temp files are removed after every outcome.
- Disk-space pre-check (`fs4::available_space`): estimate (×2 when transcoding) + 200 MB.
- yt-dlp stderr → plain-language message (`classify`: disk full, bot check,
  age-restricted, private, unavailable, bad link, no format, network, unknown);
  the full text goes to the log.
- Statuses: `none | ready | failed | missing` are stored; "downloading" lives in the job.
  Startup turns stale `downloading` into `failed` and `ready`-without-file into
  `missing`; `get_recipe` does the same check.
- First download installs the tools automatically (stage "Setting up the video
  downloader"); Settings → **Update downloader** re-checks all three.

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

As built (`tools.rs`): "latest" is resolved without the GitHub API by reading the
redirect of `…/releases/latest` (→ `/releases/tag/<tag>`), and for martin-riedl the
redirect of `/redirect/latest/macos/<arch>/release/ffmpeg.zip` (→ build folder
`<ts>_<version>`; its `HEAD` answers 404, so a non-following `GET` is used).
FFmpeg-Builds' `latest` tag is rolling, so its asset checksum identifies the build.
`bin/tools.json` records `source_id` + display version per package; Update downloader
reinstalls a package only when its `source_id` changed. Downloads stream to `bin/tmp/`
with SHA-256 computed on the fly (≤ 400 MB), are verified, unpacked (zip / tar.xz, only
the wanted executables), made executable, then renamed into place.

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

### Web import (as built)

Brief, Phase 5: fetch a page in Rust, read its schema.org Recipe JSON-LD, pre-fill the
editor for review (never save automatically), download the main image; if there is no
structured data, say so and keep just the URL.

- **Entry:** "Import from web" in the top bar opens a dialog (pasting into the empty field
  starts at once). Success navigates to `/recipes/new` with router state
  `{ webImport }`; the editor starts from a blank form filled with the result, so it
  counts as unsaved (leaving asks first). No recipe data → the dialog says so and offers
  "Create recipe with just the link" (only `source_url` filled).
- **Fetch (`web_import.rs`):** http/https only, no credentials, a missing scheme means
  https. Addresses on this computer or the local network are refused (`localhost`,
  `.local`, private/link-local/CGNAT IPs, IPv6 ULA/link-local, IPv4-mapped) — for the
  page, every redirect hop and the image, since those are chosen by the page. DNS
  names that resolve to private IPs aren't caught (acceptable for a desktop app).
  Browser-like User-Agent and Accept headers (Q16); a fresh client per import with an
  in-memory cookie jar (consent redirects); 10 s connect, 15 s page, 20 s image timeouts;
  page ≤ 5 MB (HTML/XHTML/text only), image ≤ 10 MB. Charset: header, else
  `<meta charset>`, else UTF-8 (`encoding_rs`). gzip/brotli accepted.
- **Extraction (`jsonld.rs`, pure):** every `<script type="application/ld+json">` block
  (`scraper`); broken JSON gets one repair pass (raw line breaks inside strings).
  Recipes are found anywhere in the tree (`@graph`, arrays, nested `mainEntity`), `@type`
  as string/list/`schema:`/URL form; the first Recipe with ingredients wins. `@id`
  references are followed (Yoast's image graph). Text values: HTML tags stripped with
  block tags as line breaks, entities decoded.
  - Ingredients: `recipeIngredient` (or legacy `ingredients`), list or newline string →
    `ingredient_parser::parse_line`.
  - Steps: string (split on lines; leading "1." / "Step 2:" removed), `HowToStep`
    (`text`, else `name`), `HowToSection` → its name goes in front of its first step
    ("For the sauce: Melt the butter…"), since steps have no headings.
  - Times: ISO 8601 (`duration.rs`, also plain "1 hr 30 mins"). Cook = `cookTime`, else
    `totalTime − prepTime`; a lone `totalTime` is dropped (the editor has no total).
  - Servings: first whole number of `recipeYield` (number, string or list).
  - Nutrition: leading number of calories/protein/carbs/fat ("1,200 kcal", "3,5 g").
  - Images: string/`ImageObject`/list; largest by width×height first, else page order;
    relative URLs resolved; up to 3 tried; the bytes decide the type (jpg/png/webp/gif/avif).
  - Labels (Q17): `recipeCategory`, `recipeCuisine`, `keywords` are matched
    case-insensitively against **existing** categories, then tags; new labels are never
    created from a page (keyword lists are SEO noise).
  - Video: the first YouTube URL in `video` (`contentUrl`/`embedUrl`/`url`) fills the
    Video field; it downloads only when the user clicks Download (Q18).
  - Not imported: ratings (the app's rating is the user's own), author, total-only times.
- Warnings shown on the editor notice: photo couldn't be downloaded, no ingredients, no
  steps. Errors are plain language (not found, refused/blocked, too slow, unreachable,
  not a web page, too large); details go to the log.
- Tests: fixtures in `src-tauri/tests/fixtures/` are cut-down pages modelled on common
  site shapes (WordPress + Yoast + WP Recipe Maker, a large publisher's top-level array,
  a newspaper's multiple blocks, a hand-built blog with broken JSON, an article without a
  recipe) — not copies of real pages. Live check:
  `RECIPIO_IMPORT_URLS="<url> <url>" cargo test -- --ignored live_import --nocapture`.
  Checked 2026-10-23 against 13 real recipe sites (all read correctly) and a page without a recipe.

### Cooking mode (as built)

- Route `/recipes/:id/cook`, outside the app layout (no top bar); "Start cooking" on the
  recipe page. On open: OS full screen (`setFullscreen`, undone on exit unless the
  window was already full screen) and `set_keep_awake(true)`.
- Keep-awake: `keepawake` crate, display + idle sleep, held in `AppState` until cooking
  mode closes (or the app exits). The command is synchronous on purpose: Tauri runs sync
  commands on the main thread, and Windows' `SetThreadExecutionState` belongs to the
  calling thread. Failure is logged only (the screen may dim).
- One step at a time in large text, "Step n of m", Previous/Next (Done on the last step),
  a clickable dot per step. Keys: ← → ↑ ↓ PageUp/PageDown move, Home/End jump, I toggles
  the ingredients panel, Esc exits. Ingredients panel (side, stacked on narrow windows)
  with tick-off checkboxes; ticks aren't saved.

### Backup (as built, `backup.rs`)

- Zip with `recipio-backup.json` (own format: `format`, `version` 1, app version,
  created_at, all categories and tags incl. unused, recipes with numeric quantities,
  timestamps, video status, media entry names) plus `media/photos/…` and
  `media/videos/…` stored uncompressed (they're already compressed). Written to
  `<name>.part`, renamed when complete; exports can be cancelled. Videos are optional
  (Q19).
- Restore = full replace after a warning that shows both libraries' counts (Q5). Order:
  read + check the manifest (each recipe through `recipes::validate`; media entry names
  must be exactly `media/photos/<uuid>.<photo ext>` / `media/videos/<uuid>.mp4`, nothing
  else is unpacked) → disk-space check → unpack to `media/restore/` (never more bytes
  than the zip declares) → one DB transaction replaces everything (labels, recipes, FTS)
  keeping ids and timestamps → swap `photos/`+`videos/` by folder renames. A failure
  before the swap leaves the library untouched. Leftover `restore*/` folders are removed
  at startup. Refused while a video download is running (Q20).
- Video status on restore: file included → ready; left out but was ready/missing →
  missing (Re-download offered); failed stays failed.
- Newer backup versions are refused with "update the app first".

### Video bookmarks (owner request after Phase 6)

- Table `video_bookmarks` (migration 0002): recipe_id (cascade), whole `seconds`, optional
  `label` (≤ 40 chars). At most 12 per recipe, no two at the same second, not past the
  video's end (the player's duration is passed in). `Recipe.bookmarks` is sorted by time
  with Rust-formatted `time_text`.
- Recipe page, under the player: bookmarks as chips left to right in time order (click =
  seek and play, × = remove at once), then **Add bookmark** → inline form pre-filled with
  the player's position. Times accepted: `2:15`, `1:02:03`, `90`, `1m30s`
  (`duration::timestamp`).
- Bookmarks belong to the video: kept when the file is removed/re-downloaded, cleared when
  the recipe's YouTube link changes to a different video id (`save_recipe` and a finished
  download both check, via `video::youtube_id`). Saving the editor doesn't touch them;
  they don't change `updated_at`. Included in backups and share files.

### Sharing recipes (owner request after Phase 6)

- Top bar → **Share recipes** (`/share`): find/tick recipes (Select all /
  Select these), see the photo/video size, optionally include downloaded videos (off by
  default — the friend can download from the YouTube link), **Save share file…** →
  `<title or "N recipes">.recipio`.
- File = the backup zip layout with `format: "recipio-share"`: chosen recipes only, no
  star ratings (personal), labels travel on each recipe, bookmarks included.
- Top bar → **Import shared recipes** (`/share/import`): choose a `.recipio`
  (a backup `.zip` also works) → list with checkboxes; recipes whose id is already in the
  library are unticked and marked "Already in your library" (ticking adds a copy with a
  new id). **Add N recipes to my library** never changes or removes anything: files are
  unpacked to `tmp/`, moved under the final ids, then one DB transaction adds the recipes
  (created/updated = now, missing labels created). A failure removes the moved files.
  Video status: file included → ready; otherwise none (Download video is offered).
- A share file can't be used with Restore (that needs a full backup), so a friend's file
  can never replace a library.

### Recipe PDF (owner request after Phase 6)

- Recipe page → **PDF** (printer icon) → save dialog → `export_pdf` writes the file and
  opens it in the default viewer. Rust `pdf.rs`: `krilla` writes the PDF (pure Rust,
  embeds JPEG/PNG/WebP/GIF; AVIF photos are left out); the layout (word wrap, page
  breaks, keeping headings with what follows) is done in `pdf.rs`, measuring text with
  `rustybuzz` (krilla's own shaper) so wrapped lines fit.
- Content: title, servings/prep/cook/rating, categories and tags, description, photo
  (≤ 260 pt tall), ingredients (section headings, bullets), numbered steps with hanging
  indent, notes, nutrition, source URL; footer "title · Page n of m". **Never the video**:
  no file, no YouTube link, no bookmarks (Q24).
- Paper: US Letter for US/CA/MX/PH/PR/CL/CO/VE locales (from the UI's
  `navigator.language`), A4 otherwise.
- Font: Noto Sans Regular/Bold bundled in `src-tauri/assets/fonts/` (SIL OFL 1.1,
  license file alongside; credit it in the README). krilla subsets it into each PDF.
- Live check: `RECIPIO_PDF_DB=<db copy> RECIPIO_PDF_MEDIA=<media dir> RECIPIO_PDF_ID=<id>
RECIPIO_PDF_OUT=<file.pdf> cargo test -- --ignored live_pdf`.

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

| Crate                                                            | Why                                                       |
| ---------------------------------------------------------------- | --------------------------------------------------------- |
| `tauri` 2, `tauri-build`                                         | Framework                                                 |
| `tauri-plugin-dialog`, `tauri-plugin-opener`, `tauri-plugin-log` | Official plugins: file pickers, open URLs, log file       |
| `rusqlite` (`bundled`)                                           | SQLite incl. FTS5, no system dependency; simple sync API  |
| `rusqlite_migration`                                             | Versioned migrations via `user_version`                   |
| `serde`, `serde_json`                                            | Serialization                                             |
| `thiserror`                                                      | Error types                                               |
| `uuid` (v4)                                                      | Recipe IDs                                                |
| `chrono`                                                         | Timestamps                                                |
| `reqwest` (`rustls-tls`, `stream`, no default features)          | HTTP for tools, pages, images; no OpenSSL dependency      |
| `sha2`, `hex`                                                    | Checksum verification                                     |
| `zip`                                                            | Backup archives; Windows ffmpeg archive                   |
| `tar`, `xz2`                                                     | Linux ffmpeg `.tar.xz` archives                           |
| `scraper`                                                        | Locate `<script type="application/ld+json">` blocks       |
| `encoding_rs`, `url`                                             | Page charsets; URL host checks (both already via reqwest) |
| `fs4`                                                            | Free disk space check                                     |
| `keepawake`                                                      | Keep screen awake in cooking mode                         |
| `ts-rs`                                                          | Generate TS types from Rust structs                       |
| `tokio` (via Tauri runtime; `process`, `io-util`)                | Child process streaming                                   |
| `log`                                                            | Logging facade                                            |
| dev: `tempfile`, `pretty_assertions`                             | Tests                                                     |

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

### Phase 8 (as built): Friends

Owner decision Q25: no servers, no in-app chat; recipes travel through services people
already use. Friends use macOS and Windows.

**How a recipe gets to a friend**

| You're on | Friend's method | What happens                                                                                            |
| --------- | --------------- | ------------------------------------------------------------------------------------------------------- |
| macOS     | Messages        | Messages opens a new message to the friend with the `.recipio` file attached; you type, send            |
| macOS     | Email           | Mail opens a new email to the friend, subject "Recipes from <you>", file attached                       |
| macOS     | (any)           | "Other…" opens the system share menu (AirDrop, Mail, Messages, Notes…)                                  |
| Windows   | Email           | a new email to the friend opens in the default mail app; the file is shown in Explorer to drag in (Q26) |
| any       | fallback        | "Save file…" / "Show in Finder/Explorer" (e.g. Gmail in a browser: attach it yourself)                  |

Messages only reaches Apple devices, so Windows friends are set to Email. macOS uses
`NSSharingService` (Messages/Mail with recipient, subject, text and attachment) and
`NSSharingServicePicker` (the share menu, anchored at the Send button) via
`objc2-app-kit`, on the main thread (`sharing.rs`). If a service isn't set up (e.g. Mail
without an account), it falls back to the email draft. Windows/Linux: `mailto:` draft
(recipient, subject, body that says which file to attach) + the file revealed in its
folder. Files are written to `<app_data>/outbox/` (kept 7 days, so a Mail draft can still
find its attachment). The message text is the user's note plus, for Recipio files, how
to open it and where to get Recipio.

**How a friend receives it**

- `.recipio` is registered with Recipio (bundle `fileAssociations`, exported UTI
  `io.github.bvdon.recipio.recipes`). Clicking the attachment in Messages/Mail/Explorer
  opens Recipio straight on _Import shared recipes_ (`/share/import?file=…`). macOS
  delivers it as `RunEvent::Opened`; on Windows/Linux the file is a launch argument, and
  `tauri-plugin-single-instance` hands a second launch's arguments to the running app.
  Files are queued in Rust (`take_opened_files`) and announced with `app://open-files`,
  so a file that arrives before the UI is ready isn't lost.
- The import screen shows who it's from and their note ("From Sam: try the curry!").

**In the app**

- Top bar: one **Friends** button replaces _Share recipes_ / _Import shared recipes_.
- `/friends`: your friends (name, Messages or Email, phone/email); add/edit/remove; under
  each friend, what you sent them and what they sent you (date, recipe titles). Buttons:
  _Share recipes…_ (the existing picker, now ending in "Send to <friend> / Other… / Save
  file…"), _Share the whole book_, _Import a file…_.
- Recipe page: **Share** button (next to PDF) → menu of friends + "Other…" for that recipe.
- Received recipes remember the sender: a "From Sam" chip on the recipe page.
- Settings → Sharing and the Friends page: **Your name** (shown to friends in files you
  send); the Share dialog asks for it the first time a Recipio file is sent.
- **Share dialog** (recipe page, the picker, a friend's "Share recipes…", "Share the whole
  book" = picker with everything ticked): send to a friend / Other… / Save a file; as a
  Recipio file (videos optional) or a PDF (one PDF, each recipe on new pages); optional
  note; size estimate with an email-size warning; afterwards it says what to do next
  (e.g. "Messages is open… press Send") with a Show file button.

**Data**: migration 0003 — `friends` (id, name, method `messages|email`, address,
created_at), `share_log` (id, friend_id null, direction `sent|received`, sender name,
recipe titles JSON, file name, at), `recipes.shared_by` (TEXT, null). Share manifest
gains optional `from` and `note` (older app versions ignore them). Friends and the log
are local; they're included in backups.

**Size**: email providers cap attachments around 20–25 MB, Messages around 100 MB. Share
files keep photos small (resized to ≤ 1600 px JPEG, typically 200–400 KB; backups keep
originals); videos stay off by default. The picker shows the file size and warns above
20 MB for email ("send fewer recipes, or use AirDrop / Save file").

**Not included**: in-app chat, contacts import (manual entry for now), syncing, read
receipts, Linux-native sharing (Linux gets Save file / Show in folder).

**Testing**: Rust unit tests for the friends/log tables, manifest `from`/`note`, received
log and `shared_by`, photo resizing, friends in backups, file names, outbox pruning,
`mailto:` links and finding `.recipio` files in launch arguments. macOS reports the
Messages and Mail services available for file + text (checked with a script). The share
UI itself needs the owner; Windows compiles in CI but is untested until a Windows machine
is available.

## 6. Open questions / decisions log

| #   | Question                                                                            | Decision                                                                                                                                                       |
| --- | ----------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Q1  | Ranges ("2-3 eggs"): add nullable `quantity_max` to ingredients?                    | **Yes** — add `quantity_max`                                                                                                                                   |
| Q2  | YouTube now requires a JS runtime for yt-dlp (Deno). Also manage Deno?              | **Yes** — download Deno from denoland/deno GitHub releases, verify `.sha256sum`                                                                                |
| Q3  | Use custom range-capable `recipio-media://` protocol from the start?                | **Yes** — custom protocol from the start                                                                                                                       |
| Q4  | Checksum trust: publisher manifests over HTTPS acceptable (vs pinned hashes / GPG)? | **Yes** — publisher checksums are sufficient for v1                                                                                                            |
| Q5  | Backup import = full replace of library (with warning), not merge?                  | **Yes** — full replace after warning                                                                                                                           |
| Q6  | Unsigned/un-notarized installers acceptable for v1?                                 | **Yes** — unsigned is fine                                                                                                                                     |
| Q7  | Bundle identifier (determines app data path), e.g. `io.github.<user>.recipio`?      | **`io.github.bvdon.recipio`** (owner deferred to Claude)                                                                                                       |
| Q8  | GitHub repo: create/push at Phase 1 or Phase 7? Repo name/owner?                    | Public repo `bvdon/recipio` (timing: see CLAUDE.md status)                                                                                                     |
| Q9  | Testing scope                                                                       | macOS-only testing is fine for now; Windows/Linux via CI later                                                                                                 |
| Q10 | Servings scaling (brief, Phase 3)                                                   | **Dropped** by the owner (2026-10-22) — no servings control, no `scaling.rs`                                                                                   |
| Q11 | Quantity entry                                                                      | Typed as text (`3/4`, `1 1/2`, `½`, `2-3`), parsed in Rust (`quantity.rs`), shown as fractions (`¾`, `1½`)                                                     |
| Q12 | Paste-to-parse review                                                               | Read-only preview in a dialog, then lines are added to the editor for any fixes (no editing inside the dialog)                                                 |
| Q13 | App icon                                                                            | The logo's chef hat (lucide `ChefHat`), white on the accent terracotta; source `src-tauri/icons/icon.svg`                                                      |
| Q14 | Video format choice                                                                 | `-S res:1080` (short side) instead of `[height<=1080]`, so vertical videos stay at 1080p; H.264/AAC preferred, transcode only otherwise                        |
| Q15 | Removing a video                                                                    | Immediate (after a confirm), not on Save; the YouTube URL is kept for re-download                                                                              |
| Q16 | Web import User-Agent                                                               | Browser-like (Safari) headers; many recipe sites refuse requests that name themselves as an app                                                                |
| Q17 | Web import categories/keywords                                                      | Matched to existing categories/tags only; never creates new labels from a page                                                                                 |
| Q18 | Video named by an imported page                                                     | Fills the editor's Video field; nothing downloads until the user clicks Download                                                                               |
| Q19 | Videos in backups                                                                   | Optional ("Include downloaded videos", on by default); left-out videos restore as "missing" with Re-download                                                   |
| Q20 | Restore during a video download                                                     | Refused with a message (the download would write into the replaced library)                                                                                    |
| Q21 | Video bookmarks                                                                     | Optional label; max 12; cleared when the YouTube link changes to another video; managed on the recipe page                                                     |
| Q22 | Share file format                                                                   | `.recipio` = backup zip layout, format `recipio-share`, chosen recipes, no ratings; videos optional (off by default)                                           |
| Q23 | Importing a recipe that already exists                                              | Never overwrite: unticked by default; if ticked, added as a copy with a new id                                                                                 |
| Q24 | Recipe PDF                                                                          | Generated in Rust (krilla + Noto Sans), opened after saving; never includes the video, its link or bookmarks; Letter/A4 by locale                              |
| Q25 | Friends / sharing with friends                                                      | Option 3 (2026-10-23): native sharing (Messages/Mail/AirDrop, Windows share) + `.recipio` file association; no servers, no in-app chat. Phase 8, after Phase 7 |
| Q26 | Windows sharing                                                                     | `mailto:` draft to the friend + file shown in Explorer, instead of the Windows share panel (WinRT code that can't be tested yet)                               |
