//! The media folder (photos, videos, staging area) and the `recipio-media://` protocol
//! that serves it to the webview with HTTP Range support (needed for video seeking).

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use http::{Request, Response, StatusCode, header};
use percent_encoding::percent_decode_str;

use crate::error::{AppError, AppResult};

pub const PROTOCOL: &str = "recipio-media";

const PHOTOS: &str = "photos";
const VIDEOS: &str = "videos";
const TMP: &str = "tmp";
const PHOTO_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "avif"];
const MAX_PHOTO_BYTES: u64 = 50 * 1024 * 1024;
/// Largest body returned for one range request; players ask for the rest as needed.
const MAX_RANGE_BYTES: u64 = 4 * 1024 * 1024;
/// Files above this are never read whole: a request without `Range` gets the first
/// chunk as a partial response, which media players handle by asking for more.
const MAX_FULL_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct MediaDir {
    root: PathBuf,
}

impl MediaDir {
    /// Creates the folder structure if needed. `root` is canonicalized so later
    /// containment checks compare like with like.
    pub fn new(root: &Path) -> std::io::Result<Self> {
        for sub in [PHOTOS, VIDEOS, TMP] {
            fs::create_dir_all(root.join(sub))?;
        }
        Ok(Self {
            root: root.canonicalize()?,
        })
    }

    /// Resolves a media-relative path to an existing file inside the media folder.
    /// Rejects absolute paths, `..`, and anything (e.g. a symlink) that escapes the folder.
    pub fn resolve(&self, rel: &str) -> Option<PathBuf> {
        let rel = Path::new(rel);
        if rel.as_os_str().is_empty()
            || !rel.components().all(|c| matches!(c, Component::Normal(_)))
        {
            return None;
        }
        let path = self.root.join(rel).canonicalize().ok()?;
        (path.starts_with(&self.root) && path.is_file()).then_some(path)
    }

    /// Copies a user-chosen image into the staging area and returns its relative path.
    /// The file only becomes the recipe's photo when the recipe is saved.
    pub fn stage_photo(&self, source: &Path) -> AppResult<String> {
        let ext = source
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|e| PHOTO_EXTENSIONS.contains(&e.as_str()))
            .ok_or_else(|| {
                AppError::Validation("Please choose a JPEG, PNG, WebP, GIF or AVIF image.".into())
            })?;
        let meta = fs::metadata(source)?;
        if !meta.is_file() {
            return Err(AppError::Validation("That is not a file.".into()));
        }
        if meta.len() > MAX_PHOTO_BYTES {
            return Err(AppError::Validation(
                "That image is too large (max 50 MB).".into(),
            ));
        }
        let rel = format!("{TMP}/{}.{ext}", uuid::Uuid::new_v4());
        fs::copy(source, self.root.join(&rel))?;
        Ok(rel)
    }

    /// Moves a staged photo to `photos/<recipe_id>.<ext>` and returns that relative path.
    pub fn commit_photo(&self, recipe_id: &str, staged: &str) -> AppResult<String> {
        let staged_path = Path::new(staged);
        let in_tmp = staged_path.parent() == Some(Path::new(TMP));
        let src = self.resolve(staged).filter(|_| in_tmp).ok_or_else(|| {
            AppError::Validation(
                "The chosen photo is no longer available. Please choose it again.".into(),
            )
        })?;
        let ext = staged_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jpg");
        let rel = format!("{PHOTOS}/{recipe_id}.{ext}");
        fs::rename(src, self.root.join(&rel))?;
        Ok(rel)
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join(TMP)
    }

    /// Writes downloaded image bytes (e.g. a video thumbnail) into the staging area, like
    /// `stage_photo`.
    pub fn stage_bytes(&self, ext: &str, data: &[u8]) -> AppResult<String> {
        let rel = format!("{TMP}/{}.{ext}", uuid::Uuid::new_v4());
        fs::write(self.root.join(&rel), data)?;
        Ok(rel)
    }

    /// Moves a finished download from the staging area to `videos/<recipe_id>.mp4`
    /// (replacing any previous video) and returns that relative path.
    pub fn commit_video(&self, recipe_id: &str, file: &Path) -> AppResult<String> {
        if file.parent() != Some(self.tmp_dir().as_path()) {
            return Err(AppError::Download(format!(
                "unexpected video location {}",
                file.display()
            )));
        }
        let rel = format!("{VIDEOS}/{recipe_id}.mp4");
        fs::rename(file, self.root.join(&rel))?;
        Ok(rel)
    }

    /// The finished-but-not-yet-moved download for a recipe, if yt-dlp didn't report it.
    pub fn find_video_temp(&self, recipe_id: &str) -> Option<PathBuf> {
        let prefix = format!("{recipe_id}.dl.");
        fs::read_dir(self.tmp_dir())
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                name.starts_with(&prefix)
                    && !name.ends_with(".part")
                    && !name.ends_with(".ytdl")
                    && !name.contains(".f")
            })
    }

    /// Removes a recipe's partial download files (after success, failure or cancel).
    pub fn remove_video_temp(&self, recipe_id: &str) {
        let Ok(entries) = fs::read_dir(self.tmp_dir()) else {
            return;
        };
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{recipe_id}."))
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    /// Deletes a media file. Missing files are fine; other failures are logged, not raised,
    /// because the database change they accompany has already happened.
    pub fn remove(&self, rel: &str) {
        if let Some(path) = self.resolve(rel)
            && let Err(e) = fs::remove_file(&path)
        {
            log::warn!("could not delete {}: {e}", path.display());
        }
    }

    /// Startup housekeeping: empties the staging area and deletes photo/video files that
    /// no recipe references (e.g. left behind by a crash).
    pub fn sweep(&self, referenced: &HashSet<String>) {
        let _ = fs::remove_dir_all(self.root.join(TMP));
        let _ = fs::create_dir_all(self.root.join(TMP));
        for sub in [PHOTOS, VIDEOS] {
            let Ok(entries) = fs::read_dir(self.root.join(sub)) else {
                continue;
            };
            for entry in entries.flatten() {
                let rel = format!("{sub}/{}", entry.file_name().to_string_lossy());
                if !referenced.contains(&rel) {
                    log::info!("removing orphaned media file {rel}");
                    self.remove(&rel);
                }
            }
        }
    }
}

/// Handles one `recipio-media://` request. The URL path is the percent-encoded
/// media-relative path, as produced by `convertFileSrc(rel, "recipio-media")`.
pub fn handle(media: &MediaDir, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let encoded = request.uri().path().trim_start_matches('/');
    let rel = percent_decode_str(encoded).decode_utf8_lossy();
    let Some(path) = media.resolve(&rel) else {
        return status(StatusCode::NOT_FOUND);
    };
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok());
    match serve_file(&path, range) {
        Ok(resp) => resp,
        Err(e) => {
            log::error!("media protocol failed for {}: {e}", path.display());
            status(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

fn serve_file(path: &Path, range: Option<&str>) -> std::io::Result<Response<Vec<u8>>> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let builder = Response::builder()
        .header(header::CONTENT_TYPE, content_type(path))
        .header(header::ACCEPT_RANGES, "bytes");

    let range = match range {
        None if size > MAX_FULL_BYTES => Some("bytes=0-"),
        r => r,
    };
    let Some(range) = range else {
        let mut body = Vec::with_capacity(size as usize);
        file.read_to_end(&mut body)?;
        return Ok(builder
            .status(StatusCode::OK)
            .header(header::CONTENT_LENGTH, body.len())
            .body(body)
            .expect("valid response"));
    };

    let Some((start, end)) = parse_range(range, size) else {
        return Ok(builder
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{size}"))
            .body(Vec::new())
            .expect("valid response"));
    };
    let end = end.min(start + MAX_RANGE_BYTES - 1);
    let len = end - start + 1;
    let mut body = vec![0; len as usize];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut body)?;
    Ok(builder
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"))
        .header(header::CONTENT_LENGTH, len)
        .body(body)
        .expect("valid response"))
}

/// Parses a single `bytes=` range against a file of `size` bytes and returns the
/// inclusive `(start, end)`. Multi-range requests are answered with the first range.
fn parse_range(header: &str, size: u64) -> Option<(u64, u64)> {
    let spec = header
        .trim()
        .strip_prefix("bytes=")?
        .split(',')
        .next()?
        .trim();
    let (start, end) = spec.split_once('-')?;
    let (start, end) = match (start.trim(), end.trim()) {
        ("", suffix) => {
            let n: u64 = suffix.parse().ok()?;
            if n == 0 {
                return None;
            }
            (size.saturating_sub(n), size.checked_sub(1)?)
        }
        (s, "") => (s.parse().ok()?, size.checked_sub(1)?),
        (s, e) => (
            s.parse().ok()?,
            e.parse::<u64>().ok()?.min(size.checked_sub(1)?),
        ),
    };
    (start <= end && start < size).then_some((start, end))
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mp4" | "m4v") => "video/mp4",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("avif") => "image/avif",
        _ => "application/octet-stream",
    }
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(code)
        .body(Vec::new())
        .expect("valid response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ranges() {
        assert_eq!(parse_range("bytes=0-1", 100), Some((0, 1)));
        assert_eq!(parse_range("bytes=10-", 100), Some((10, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-500", 100), Some((0, 99)));
        assert_eq!(parse_range("bytes=50-500", 100), Some((50, 99)));
        assert_eq!(parse_range("bytes=0-1, 5-6", 100), Some((0, 1)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=5-2", 100), None);
        assert_eq!(parse_range("bytes=-0", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
        assert_eq!(parse_range("bytes=0-", 0), None);
    }

    fn setup() -> (tempfile::TempDir, MediaDir) {
        let dir = tempfile::tempdir().unwrap();
        let media = MediaDir::new(&dir.path().join("media")).unwrap();
        fs::write(
            dir.path().join("media/videos/a.mp4"),
            (0..=255u8).collect::<Vec<_>>(),
        )
        .unwrap();
        fs::write(dir.path().join("secret.txt"), "nope").unwrap();
        (dir, media)
    }

    fn get(media: &MediaDir, uri: &str, range: Option<&str>) -> Response<Vec<u8>> {
        let mut req = Request::builder().uri(uri);
        if let Some(r) = range {
            req = req.header(header::RANGE, r);
        }
        handle(media, &req.body(Vec::new()).unwrap())
    }

    #[test]
    fn serves_full_and_partial_content() {
        let (_dir, media) = setup();
        let full = get(&media, "recipio-media://localhost/videos%2Fa.mp4", None);
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.body().len(), 256);
        assert_eq!(full.headers()[header::CONTENT_TYPE], "video/mp4");

        let part = get(
            &media,
            "http://recipio-media.localhost/videos%2Fa.mp4",
            Some("bytes=10-19"),
        );
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(part.body(), &(10..20u8).collect::<Vec<_>>());
        assert_eq!(part.headers()[header::CONTENT_RANGE], "bytes 10-19/256");

        let bad = get(
            &media,
            "recipio-media://localhost/videos%2Fa.mp4",
            Some("bytes=300-"),
        );
        assert_eq!(bad.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    }

    #[test]
    fn refuses_paths_outside_media() {
        let (_dir, media) = setup();
        for uri in [
            "recipio-media://localhost/..%2Fsecret.txt",
            "recipio-media://localhost/videos%2F..%2F..%2Fsecret.txt",
            "recipio-media://localhost/%2Fetc%2Fpasswd",
            "recipio-media://localhost/videos",
        ] {
            assert_eq!(
                get(&media, uri, None).status(),
                StatusCode::NOT_FOUND,
                "{uri}"
            );
        }
    }

    #[test]
    fn stage_commit_and_sweep_photo() {
        let (dir, media) = setup();
        let src = dir.path().join("Pic.PNG");
        fs::write(&src, b"png").unwrap();
        let staged = media.stage_photo(&src).unwrap();
        assert!(staged.starts_with("tmp/") && staged.ends_with(".png"));
        let id = "6f1c1f0e-2b9a-4d4e-9a53-0d3f4b7d8c11";
        let rel = media.commit_photo(id, &staged).unwrap();
        assert_eq!(rel, format!("photos/{id}.png"));
        assert!(media.resolve(&rel).is_some());
        assert!(media.commit_photo(id, "videos/a.mp4").is_err());

        media.sweep(&HashSet::from([rel.clone()]));
        assert!(media.resolve(&rel).is_some());
        assert!(media.resolve("videos/a.mp4").is_none());

        let txt = dir.path().join("x.txt");
        fs::write(&txt, b"x").unwrap();
        assert!(matches!(
            media.stage_photo(&txt),
            Err(AppError::Validation(_))
        ));
    }
}
