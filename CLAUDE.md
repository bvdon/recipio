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

| Phase                   | State                                        |
| ----------------------- | -------------------------------------------- |
| 1 Foundation            | done, committed; **awaiting owner approval** |
| 2 Organize              | not started                                  |
| 3 Ingredients plus      | not started                                  |
| 4 Video                 | not started                                  |
| 5 Web import            | not started                                  |
| 6 Cooking mode + backup | not started                                  |
| 7 Packaging             | not started                                  |

Phase 1 delivered: scaffold, migration 0001 (full schema incl. categories/tags/FTS),
recipe CRUD commands, photo staging/commit, `recipio-media://` protocol with Range
support (already used for photos), library grid/list, view page, editor (keyboard
ingredient rows, section headings, drag reorder, steps, rating, nutrition, sources),
unsaved-changes guard, delete with confirmation (also deletes media files), log file.

Phase-1 stopgaps to replace later:

- Quantity input is parsed in TS (`parseQty` in `IngredientsEditor.tsx`: decimals and
  `2-3` only) and displayed as decimals (`formatQuantity` in `common.tsx`). Phase 3 moves
  parsing/fraction formatting to Rust.
- Categories/tags are in the schema and read into `Recipe`, but there is no UI or save
  path for them yet (Phase 2). Library sorts by `updated_at DESC` only.

## Environment / gotchas

- Dev machine: macOS arm64, rustc 1.98, node 25, npm 11 (no pnpm — use npm).
- **Linker gotcha on this Mac:** the Command Line Tools linker can't read the beta
  macOS 27 SDK that `xcrun` picks by default ("tapi error: unknown architecture"). Prefix
  Rust/Tauri commands with `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk`
  (→ 26.5 SDK). Not a repo issue; don't put it in project config.
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
- `src-tauri/src/db/` — `open` + migrations; `recipes.rs` repository (validate, upsert, delete, FTS reindex).
- `src-tauri/src/media.rs` — media folder, photo staging, orphan sweep, `recipio-media://` handler + range parsing.
- `src-tauri/src/models.rs` — shared types (ts-rs exported).
- `src-tauri/src/error.rs` — `AppError` → `{ kind, message }`, logs detail on serialize.
- `src/api.ts` — typed `invoke` wrappers, `mediaUrl()`, query keys.
- `src/pages/` — `Library`, `RecipeView`, `RecipeEditor`. `src/components/` — editors, `SortableList`, common bits.

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
- Hash routing (`createHashRouter`); routes: `/`, `/recipes/new`, `/recipes/:id`, `/recipes/:id/edit`.
