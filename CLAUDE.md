# Recipio — Claude context

Cross-platform desktop recipe app. Tauri v2 + Rust core + React/TS (Vite).
Local-only storage (SQLite + media folder in the OS app data dir). MIT, public GitHub repo
`bvdon/recipio`.

**Read first when resuming:**

1. `PROMPT.md` — the owner's original build brief (requirements, phases, quality bar).
   Local only, gitignored (contains personal details); if missing, `docs/PLAN.md` covers it.
2. `docs/PLAN.md` — architecture, data model, dependency choices, decisions log (Q1–Q26).
3. "Current status" below.

## Working rules (from the brief)

- Build in phases. At the end of each phase: run all checks, commit, push, give the owner
  exact test steps, then **stop and wait for approval** before the next phase.
- Propose alternatives instead of silently deviating from the brief.
- Business logic in Rust (DB, parsing, scaling, downloads); frontend = UI + state only.
- No telemetry, no network calls beyond those in the brief.
- Invoke external binaries with argument arrays, never a shell string. URLs/metadata are untrusted.
- Never commit secrets or downloaded binaries.
- **Owner preference:** after every phase (and whenever the app changes meaningfully), run
  `npm run dmg` so a fresh `Recipio_<version>_aarch64.dmg` sits in the project root.
  `*.dmg` is gitignored — never commit it.

## Current status

| Phase                   | State                             |
| ----------------------- | --------------------------------- |
| 1 Foundation            | done, approved                    |
| 2 Organize              | done, approved                    |
| 3 Ingredients plus      | done, approved                    |
| 4 Video                 | done, approved                    |
| 5 Web import            | done, approved                    |
| 6 Cooking mode + backup | done, approved                    |
| 6b Bookmarks + sharing  | done, approved                    |
| 7 Packaging             | done, approved (Windows untested) |
| 8 Friends (option 3)    | done, approved                    |

Phase 1 delivered: scaffold, migration 0001 (full schema incl. categories/tags/FTS),
recipe CRUD commands, photo staging/commit, `recipio-media://` protocol with Range
support (already used for photos), library grid/list, view page, editor (keyboard
ingredient rows, section headings, drag reorder, steps, rating, nutrition, sources),
unsaved-changes guard, delete with confirmation (also deletes media files), log file.

Phase 2 delivered: category/tag pickers in the editor (type-ahead, create on save),
library sidebar filters (AND), FTS prefix search, sort (title/added/updated/rating +
direction), `/organize` page (add, rename, delete, merge), clickable chips on the
recipe page that open the filtered library.

Phase 3 delivered: quantities are typed as text and parsed in Rust (`quantity.rs`:
fractions, mixed numbers, unicode ½, decimals, ranges), displayed as friendly fractions.
**Scaling was dropped by the owner** (PLAN.md Q10) — don't build it. Nutrition (also in
the brief's Phase 3) already shipped in Phase 1. Paste-to-parse: `ingredient_parser.rs`
(heuristics in PLAN.md §3) + `PasteIngredientsDialog` (preview, then add to the editor).
App icon = the logo's chef hat; regenerate with `npx tauri icon <1024px png>` from
`src-tauri/icons/icon.svg` (delete the android/ios folders it creates).

Phase 4 delivered: `tools.rs` (yt-dlp, ffmpeg+ffprobe, Deno into `<app_data>/bin`,
SHA-256 verified, `tools.json`), `video.rs` (background job per recipe: install tools if
needed → `-J` metadata/format → download → optional transcode → `videos/<id>.mp4`),
`process.rs` (spawn/kill trees, kill all on exit), editor Video section (paste a link →
auto-download, pre-fills empty title/photo), player on the recipe page, Re-download /
Retry / Cancel / Remove video, `/settings` with Update downloader. Design: PLAN.md §3.
Live check: `cd src-tauri && cargo test -- --ignored live_install` downloads the real tools
(set `RECIPIO_TOOLS_DIR` to keep them).

Phase 5 delivered: "Import from web" (top bar) → `web_import.rs` fetches the page
(browser-like headers, local-network addresses refused, size/time caps) → `jsonld.rs`
reads the schema.org Recipe (pure, fixture-tested) → ingredients through
`ingredient_parser`, times through `duration.rs`, labels matched to existing ones only,
first YouTube video pre-filled, photo staged → the new-recipe editor opens pre-filled
(router state) for review. Design: PLAN.md §3 "Web import". Live check:
`cd src-tauri && RECIPIO_IMPORT_URLS="<url> …" cargo test -- --ignored live_import --nocapture`.

Phase 6 delivered: cooking mode (`/recipes/:id/cook`, outside the layout: full screen,
one step at a time, arrow keys, ingredients panel with ticks, `set_keep_awake` via the
`keepawake` crate) and backup (`backup.rs`: zip export with optional videos, cancel,
progress; restore = full replace after a warning, staged and transactional). Settings has
both the Backup and Video downloader sections. Design: PLAN.md §3. Live check for
keep-awake: `cargo test -- --ignored live_keep_awake` while running `pmset -g assertions`.

Phase 6b (owner request before Phase 7): video bookmarks (migration 0002,
`db/bookmarks.rs`, chips + Add bookmark under the player on the recipe page) and sharing
(`/share` writes a `.recipio` file of chosen recipes via `backup::export(Kind::Share)`;
`/share/import` previews and adds recipes via `backup::add_shared`, never overwriting).
Design: PLAN.md §3 "Video bookmarks" and "Sharing recipes". Also a **PDF** button on the
recipe page (`pdf.rs`: krilla + bundled Noto Sans, never the video; PLAN.md §3 "Recipe PDF").

Phase 8 delivered: Friends (`/friends`, migration 0003: `friends`, `share_log`,
`settings`, `recipes.shared_by`; `db/friends.rs`), the Share dialog (friend / Other… /
file; Recipio file or PDF; note) with `sharing.rs` handing files to Messages/Mail
(`NSSharingService`) or the share menu on macOS and a `mailto:` draft + revealed file
elsewhere; share files carry `from`/`note` and smaller photos; `.recipio` file association

- single-instance plugin open attachments on the import screen. Design: PLAN.md §3
  "Phase 8 (as built)".

All phases are done and approved; public pre-release **v0.1.0-beta.1** is out (tag carries
the beta suffix; app version stays 0.1.0 because the Windows MSI rejects pre-release
versions). The owner is using the app and will report findings. Open: Windows/Linux not
tried by hand; installers unsigned (Q6); Windows share panel deferred (Q26).

## Environment / gotchas

- Dev machine: macOS arm64, rustc 1.98, node 25, npm 11 (no pnpm — use npm).
- The old `SDKROOT=…` linker workaround is no longer needed (fixed by the owner's
  system update, 2026-10-22). If "tapi error: unknown architecture" reappears, prefix
  Rust commands with `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk`.
- Screen capture is not permitted for Claude here; the owner tests the UI.
- Git author is repo-local: `bvdon <43088+bvdon@users.noreply.github.com>`. `gh` is
  logged in as `bvdon`.
- App data (macOS): `~/Library/Application Support/io.github.bvdon.recipio/`
  (`recipio.sqlite3`, `media/{photos,videos,tmp}`, `logs/recipio.log`).

## Commands

- Dev: `npm run tauri dev`
- macOS installer: `npm run dmg` (release build → copies the DMG to the project root, replacing the old one)
- CI: `.github/workflows/ci.yml` (frontend checks + cargo fmt/clippy/test on macOS, Windows, Linux).
  Release: `.github/workflows/release.yml` — a `v*` tag creates a draft release with universal DMG,
  NSIS/MSI, AppImage/deb; manual runs only upload workflow artifacts. `gh run list` / `gh run view`.
- Rust tests (also regenerates `src/bindings/*.ts`): `cd src-tauri && cargo test`
- Rust lint: `cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
- Frontend: `npx tsc --noEmit && npm run lint && npm run format:check` (`npm run format` to fix)

## Code map

- `src-tauri/src/lib.rs` — setup (log plugin, DB open, media sweep), protocol registration, command list.
- `src-tauri/src/commands.rs` — thin Tauri commands.
- `src-tauri/src/db/` — `open` + migrations; `recipes.rs` (validate, upsert, delete, list/search/sort, FTS reindex); `labels.rs` (categories + tags via `LabelKind`: list, create, rename, delete, merge, link).
- `src-tauri/src/media.rs` — media folder, photo staging, orphan sweep, `recipio-media://` handler + range parsing.
- `src-tauri/src/quantity.rs` — quantity parse (`"1 1/2"`, `"¾"`, `"2-3"`) and fraction formatting.
- `src-tauri/src/ingredient_parser.rs` — pasted ingredient text → editor-shaped lines (also for Phase 5 web import).
- `src-tauri/src/tools.rs` — downloader binaries: platform sources, latest-release lookup, verified install, `tools.json`.
- `src-tauri/src/video.rs` — download jobs (`video://job` events), format plan, progress, error classification, URL checks.
- `src-tauri/src/process.rs` — spawning tools (arg arrays, no console window), cancel/kill process trees.
- `src-tauri/src/web_import.rs` — page fetch (URL/host checks, caps, charset), label matching, image staging.
- `src-tauri/src/jsonld.rs` — schema.org Recipe extraction from HTML (fixtures in `src-tauri/tests/fixtures/`).
- `src-tauri/src/duration.rs` — ISO 8601 (and plain-text) durations → minutes.
- `src-tauri/src/backup.rs` — library zip export/restore (`recipio-backup.json` + media), progress events; also share files (`Kind::Share`, `preview`, `add_shared`).
- `src-tauri/src/db/bookmarks.rs` — video bookmarks (≤ 12 per recipe, time order).
- `src-tauri/src/db/friends.rs` — friends, share history, settings (`my_name`).
- `src-tauri/src/sharing.rs` — share targets, outbox, file names/messages, macOS `NSSharingService`/picker, `mailto:` fallback, `.recipio` launch-argument parsing.
- `src-tauri/src/pdf.rs` — recipe → PDF (layout + krilla drawing); fonts in `src-tauri/assets/fonts/` (OFL).
- `src-tauri/src/models.rs` — shared types (ts-rs exported).
- `src-tauri/src/error.rs` — `AppError` → `{ kind, message }`, logs detail on serialize.
- `src/api.ts` — typed `invoke` wrappers, `mediaUrl()`, query keys.
- `src/pages/` — `Library`, `RecipeView`, `RecipeEditor`, `Organize`, `Settings`, `CookingMode`, `ShareRecipes`, `ImportShared`, `Friends`. `ShareDialog` is the one place recipes are sent from. `src/components/` — editors, `LabelPicker`, `SortableList`, common bits.

## Conventions

- Recipe IDs are UUID v4 strings, created when the editor opens (before first save).
  Media files: `photos/<uuid>.<ext>`, `videos/<uuid>.mp4`; DB stores media-relative paths.
- Photos: `stage_photo` copies into `media/tmp/`; `save_recipe` with
  `PhotoChange::Replace` moves it into `photos/`. Startup empties `tmp/` and deletes
  unreferenced files in `photos/` and `videos/`.
- Media URLs: `mediaUrl(rel, updated_at)` — `?v=` busts the webview cache after a replace.
- Migrations: `src-tauri/migrations/NNNN_name.sql`, never edit an applied one — add a new one.
- FTS index (`recipes_fts`) is updated by Rust in the same transaction as recipe writes.
- Rust structs shared with the UI derive `ts_rs::TS`; generated types in `src/bindings/` are committed.
- Errors: internal detail → log file; UI gets `{ kind, message }` with a plain-language message.
- Ingredient section headings are rows with `is_heading = true`, text in `item`.
- Quantities: editor sends raw text (`IngredientInput.quantity`); `recipes::validate` parses
  into `quantity`/`quantity_max` (REAL) and returns `ValidRecipe`. Output lines carry
  Rust-formatted `quantity_text` — the UI never formats numbers itself. Live check uses
  `check_quantity` (returns bool, so bad keystrokes don't log errors).
- Hash routing (`createHashRouter`); routes: `/`, `/recipes/new`, `/recipes/:id`, `/recipes/:id/edit`, `/recipes/:id/cook` (outside the layout), `/organize`, `/settings`, `/friends`, `/share` (`?all=1`, `?friend=<id>`), `/share/import` (`?file=<path>`).
- Video: `save_recipe` never touches video fields; the job writes them when it finishes
  (and `save_recipe` links a file that finished before the first save). The UI combines
  the saved `video_status` with the live job (`useVideoDownload`). A finished job marks
  the recipe query stale with `refetchType: "none"` so an open editor isn't reset.
- App data also has `bin/` (tools + `tools.json` + `cache/` + `tmp/`) and `outbox/` (files
  handed to Messages/Mail; pruned after 7 days at startup).
- Web import never saves: the result travels as router state (`ImportState`) to
  `/recipes/new`, whose form starts from it (so it's "unsaved"). The new-recipe id query is
  keyed by `location.key`, so each visit to `/recipes/new` gets a fresh id.
- Library search/filters live in the URL (`?q=…&c=<category id>&t=<tag id>`, repeatable) so
  Back restores them; view mode and sort are per-machine prefs in `localStorage`.
- Categories and tags share one code path (`LabelKind`). Names are unique case-insensitively,
  whitespace-collapsed, ≤ 60 chars. Unused labels are kept (count 0) until deleted on
  `/organize`; the library sidebar hides them. Renaming/merging does not bump `updated_at`.
- Search: each word becomes a quoted FTS5 prefix term (`"word"*`), ANDed — user input is
  never passed to MATCH raw. Labels are not in the FTS index (filters cover them).
