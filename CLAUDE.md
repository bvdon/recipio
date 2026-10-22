# Recipio — Claude context

Cross-platform desktop recipe app. Tauri v2 + Rust core + React/TS (Vite).
Local-only storage (SQLite + media folder in the OS app data dir). MIT, public GitHub repo
`bvdon/recipio`.

**Read first when resuming:**

1. `PROMPT.md` — the owner's original build brief (requirements, phases, quality bar).
   Local only, gitignored (contains personal details); if missing, `docs/PLAN.md` covers it.
2. `docs/PLAN.md` — architecture, data model, dependency choices, decisions log (Q1–Q9).
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

| Phase                   | State                                    |
| ----------------------- | ---------------------------------------- |
| 1 Foundation            | done, approved                           |
| 2 Organize              | done, approved                           |
| 3 Ingredients plus      | fractions done; paste-to-parse remaining |
| 4 Video                 | not started                              |
| 5 Web import            | not started                              |
| 6 Cooking mode + backup | not started                              |
| 7 Packaging             | not started                              |

Phase 1 delivered: scaffold, migration 0001 (full schema incl. categories/tags/FTS),
recipe CRUD commands, photo staging/commit, `recipio-media://` protocol with Range
support (already used for photos), library grid/list, view page, editor (keyboard
ingredient rows, section headings, drag reorder, steps, rating, nutrition, sources),
unsaved-changes guard, delete with confirmation (also deletes media files), log file.

Phase 2 delivered: category/tag pickers in the editor (type-ahead, create on save),
library sidebar filters (AND), FTS prefix search, sort (title/added/updated/rating +
direction), `/organize` page (add, rename, delete, merge), clickable chips on the
recipe page that open the filtered library.

Phase 3 so far: quantities are typed as text and parsed in Rust (`quantity.rs`:
fractions, mixed numbers, unicode ½, decimals, ranges), displayed as friendly fractions.
**Scaling was dropped by the owner** (PLAN.md Q10) — don't build it. Nutrition (also in
the brief's Phase 3) already shipped in Phase 1. Remaining: paste-to-parse ingredients.

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
- Rust tests (also regenerates `src/bindings/*.ts`): `cd src-tauri && cargo test`
- Rust lint: `cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
- Frontend: `npx tsc --noEmit && npm run lint && npm run format:check` (`npm run format` to fix)

## Code map

- `src-tauri/src/lib.rs` — setup (log plugin, DB open, media sweep), protocol registration, command list.
- `src-tauri/src/commands.rs` — thin Tauri commands.
- `src-tauri/src/db/` — `open` + migrations; `recipes.rs` (validate, upsert, delete, list/search/sort, FTS reindex); `labels.rs` (categories + tags via `LabelKind`: list, create, rename, delete, merge, link).
- `src-tauri/src/media.rs` — media folder, photo staging, orphan sweep, `recipio-media://` handler + range parsing.
- `src-tauri/src/quantity.rs` — quantity parse (`"1 1/2"`, `"¾"`, `"2-3"`) and fraction formatting.
- `src-tauri/src/models.rs` — shared types (ts-rs exported).
- `src-tauri/src/error.rs` — `AppError` → `{ kind, message }`, logs detail on serialize.
- `src/api.ts` — typed `invoke` wrappers, `mediaUrl()`, query keys.
- `src/pages/` — `Library`, `RecipeView`, `RecipeEditor`, `Organize`. `src/components/` — editors, `LabelPicker`, `SortableList`, common bits.

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
- Hash routing (`createHashRouter`); routes: `/`, `/recipes/new`, `/recipes/:id`, `/recipes/:id/edit`, `/organize`.
- Library search/filters live in the URL (`?q=…&c=<category id>&t=<tag id>`, repeatable) so
  Back restores them; view mode and sort are per-machine prefs in `localStorage`.
- Categories and tags share one code path (`LabelKind`). Names are unique case-insensitively,
  whitespace-collapsed, ≤ 60 chars. Unused labels are kept (count 0) until deleted on
  `/organize`; the library sidebar hides them. Renaming/merging does not bump `updated_at`.
- Search: each word becomes a quoted FTS5 prefix term (`"word"*`), ANDed — user input is
  never passed to MATCH raw. Labels are not in the FTS index (filters cover them).
