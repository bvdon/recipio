# Recipio

A local-first desktop recipe app for macOS, Windows and Linux. Your recipes, photos and
videos live on your own computer: no accounts, no cloud, no telemetry.

Built with [Tauri v2](https://tauri.app), Rust and React.

## Features

- **Recipe library**: photos, structured ingredients (with section headings and
  fractions like `1 1/2` → 1½), steps, notes, nutrition, star ratings.
- **Find things**: full-text search, categories and tags (combined filters), sorting.
- **Paste-to-parse**: paste a block of ingredient text and Recipio splits it into
  quantity, unit, ingredient and preparation note.
- **Import from the web**: paste a recipe page's address; Recipio reads the recipe data
  most sites publish (schema.org JSON-LD) and fills in the editor for you to check.
- **Offline YouTube videos**: paste a link and the video is downloaded (up to 1080p,
  H.264/AAC MP4) and plays inside the app, with bookmarks to jump to sections.
- **Cooking mode**: full screen, one step at a time, arrow keys, ingredients to tick off,
  and the screen stays awake.
- **PDF**: save any recipe as a PDF (text and photo) to print or send.
- **Backup**: the whole library, with photos and (optionally) videos, in one zip file.
- **Share with friends**: pick recipes and save them as a `.recipio` file; your friend
  adds them with _Import shared recipes_. Nothing in their library is overwritten.

## Install

Download the installer for your system from the
[Releases page](https://github.com/bvdon/recipio/releases).

The installers are **not code-signed** yet, so your system will warn the first time:

### macOS (Apple silicon and Intel)

1. Open `Recipio_<version>_universal.dmg` and drag Recipio to Applications.
2. Open Recipio. macOS says it can't verify the developer; click **Done**.
3. Open **System Settings → Privacy & Security**, scroll down and click **Open Anyway**
   next to the Recipio message, then confirm.

Alternatively, in Terminal: `xattr -dr com.apple.quarantine /Applications/Recipio.app`.

### Windows 10/11

Run `Recipio_<version>_x64-setup.exe` (or the `.msi`). If SmartScreen shows _Windows
protected your PC_, click **More info → Run anyway**. Recipio uses Microsoft Edge
WebView2, which is part of Windows 10/11; the installer fetches it if it's missing.

### Linux

- **Debian/Ubuntu**: `sudo apt install ./Recipio_<version>_amd64.deb`. This also installs
  the GStreamer plugins needed for video playback.
- **AppImage** (any distribution): `chmod +x Recipio_<version>_amd64.AppImage` and run it.
  The AppImage includes GStreamer.

Video playback on Linux uses GStreamer through WebKitGTK. If videos don't play (for
example when building from source), install the GStreamer _base_, _good_, _bad_ and
_libav_ plugins. Package names vary by distribution:

| Distribution  | Packages                                                                                                                          |
| ------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| Debian/Ubuntu | `gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav`                                 |
| Fedora        | `gstreamer1-plugins-base gstreamer1-plugins-good gstreamer1-plugins-bad-free gstreamer1-plugin-libav` (RPM Fusion for full H.264) |
| Arch          | `gst-plugins-base gst-plugins-good gst-plugins-bad gst-libav`                                                                     |

## Video downloads

Video downloading is intended for **personal, offline use of videos you have access to**.
Please respect the terms of the sites you download from and the rights of creators.

Recipio doesn't bundle a downloader. The first time you download a video, it fetches its
own copies of [yt-dlp](https://github.com/yt-dlp/yt-dlp), [FFmpeg](https://ffmpeg.org)
(with ffprobe) and [Deno](https://deno.com) from their official release pages into its
data folder, and checks each file against the publisher's SHA-256 checksum before using
it. **Settings → Update downloader** fetches newer versions (YouTube changes often, so
update when downloads start failing).

## Your data

Everything is stored in one folder:

| OS      | Path                                                     |
| ------- | -------------------------------------------------------- |
| macOS   | `~/Library/Application Support/io.github.bvdon.recipio/` |
| Windows | `%APPDATA%\io.github.bvdon.recipio\`                     |
| Linux   | `~/.local/share/io.github.bvdon.recipio/`                |

It holds `recipio.sqlite3` (the database), `media/` (photos and videos), `bin/` (the
downloader tools) and `logs/` (a log file, useful when reporting a problem).

Recipio only goes online for what you ask it to do: downloading a video (YouTube, and the
downloader tools from GitHub/their publishers), and importing a recipe page with its
photo. There are no analytics or update checks.

Use **Settings → Backup** to export the whole library to a zip file, and restore it on
the same or another computer (restoring replaces the library, after a warning).

## Development

Prerequisites: Rust (stable), Node.js 20+ with npm, and the
[Tauri v2 system prerequisites](https://tauri.app/start/prerequisites/) for your OS. On
Debian/Ubuntu:

```sh
sudo apt install libwebkit2gtk-4.1-dev build-essential libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev
```

Run the app:

```sh
npm install
npm run tauri dev
```

Checks (the same ones CI runs on macOS, Windows and Linux):

```sh
cd src-tauri
cargo test                                   # also regenerates src/bindings/*.ts
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cd ..
npx tsc --noEmit && npm run lint && npm run format:check
```

Some tests talk to the real network and only run when asked:

```sh
cd src-tauri
cargo test -- --ignored live_install                       # download the real video tools
RECIPIO_IMPORT_URLS="https://…" cargo test -- --ignored live_import --nocapture
```

The architecture, data model and design decisions are in
[`docs/PLAN.md`](docs/PLAN.md); [`CLAUDE.md`](CLAUDE.md) has a code map and conventions.

### Building installers

```sh
npm run tauri build                                          # installers for this OS
npm run tauri build -- --target universal-apple-darwin       # universal macOS app/DMG
npm run dmg                                                  # macOS: DMG copied to the repo root
```

The universal macOS build needs both Rust targets:
`rustup target add aarch64-apple-darwin x86_64-apple-darwin`. Output goes to
`src-tauri/target/<target>/release/bundle/`.

**Releases** are built by GitHub Actions (`.github/workflows/release.yml`): push a tag
such as `v0.1.0` and a draft release with the macOS, Windows and Linux installers
appears on the Releases page; check it and click _Publish_. Running the workflow by hand
(Actions → Release → Run workflow) builds the installers as downloadable artifacts
without creating a release.

## Credits

- [Tauri](https://tauri.app), [React](https://react.dev),
  [TanStack Query](https://tanstack.com/query), [dnd kit](https://dndkit.com),
  [Lucide](https://lucide.dev) icons (ISC).
- [krilla](https://github.com/LaurenzV/krilla) for PDFs.
- [Noto Sans](https://notofonts.github.io) font, bundled for PDFs, © The Noto Project
  Authors, [SIL Open Font License 1.1](src-tauri/assets/fonts/OFL.txt).
- Downloaded at runtime, not distributed with Recipio: yt-dlp (Unlicense), FFmpeg (GPL
  builds from [yt-dlp/FFmpeg-Builds](https://github.com/yt-dlp/FFmpeg-Builds) and
  [Martin Riedl](https://ffmpeg.martin-riedl.de)), Deno (MIT).

## License

[MIT](LICENSE)
