# Recipio

A local-first desktop recipe app for macOS, Windows and Linux, built with Tauri v2,
Rust and React.

> **Status:** early development (Phase 1 of 7: foundation). See [`docs/PLAN.md`](docs/PLAN.md).

## Features (planned for v1)

- Recipe library with photos, full-text search, categories, tags and sorting
- Structured, keyboard-friendly ingredient entry with paste-to-parse and servings scaling
- YouTube videos downloaded for offline, in-app playback (up to 1080p, H.264/AAC MP4)
- Import from recipe web pages via schema.org JSON-LD
- Cooking mode and zip backup/restore

All data stays on your machine: a SQLite database and a media folder in the OS app data
directory. No accounts, no telemetry.

## Development

Prerequisites: Rust (stable), Node.js 20+, and the
[Tauri v2 system prerequisites](https://tauri.app/start/prerequisites/) for your OS.

```sh
npm install
npm run tauri dev        # run the app
```

Checks:

```sh
cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
npm run lint && npm run format:check
```

`cargo test` also regenerates the TypeScript types in `src/bindings/` from the Rust structs.

## Data location

| OS      | Path                                                     |
| ------- | -------------------------------------------------------- |
| macOS   | `~/Library/Application Support/io.github.bvdon.recipio/` |
| Windows | `%APPDATA%\io.github.bvdon.recipio\`                     |
| Linux   | `~/.local/share/io.github.bvdon.recipio/`                |

## License

[MIT](LICENSE)
