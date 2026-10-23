//! Background YouTube downloads: one job per recipe, reported to the UI through
//! `video://job` events. A job installs the tools if needed, reads the video's metadata
//! (title, thumbnail, duration), downloads H.264/AAC MP4 at most 1080p, transcodes with
//! ffmpeg only when no such streams exist, and moves the file to `videos/<id>.mp4`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::watch;
use ts_rs::TS;

use crate::AppState;
use crate::db::recipes;
use crate::duration;
use crate::error::{AppError, AppResult};
use crate::models::VideoStatus;
use crate::process;

pub const EVENT: &str = "video://job";

/// H.264 + AAC first (plays everywhere, no transcode); anything else is transcoded.
/// `-S res:1080` makes "best" mean the largest resolution up to 1080p, measured on the
/// short side so vertical videos aren't pushed down to 480p.
const FORMAT: &str = "bv*[vcodec^=avc1]+ba[acodec^=mp4a]/b[vcodec^=avc1][acodec^=mp4a]/bv*+ba/b";
const SORT: &str = "res:1080";
const MAX_SHORT_SIDE: u64 = 1080;
const MAX_THUMBNAIL_BYTES: usize = 10 * 1024 * 1024;
/// Minimum free space kept on top of the estimated download size.
const SPACE_MARGIN: u64 = 200 * 1024 * 1024;
const EMIT_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum VideoStage {
    /// First use: downloading yt-dlp, ffmpeg and Deno.
    InstallingTools,
    /// Reading title, thumbnail and available formats.
    Preparing,
    Downloading,
    /// Converting to H.264/AAC because the video had no such streams.
    Transcoding,
    Done,
    Failed,
    Cancelled,
}

impl VideoStage {
    fn is_running(self) -> bool {
        !matches!(
            self,
            VideoStage::Done | VideoStage::Failed | VideoStage::Cancelled
        )
    }
}

/// What the video turned out to be; the editor uses it to pre-fill title and photo.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct VideoInfo {
    pub title: String,
    pub duration_seconds: Option<f64>,
    /// Staged thumbnail (media-relative, like `stage_photo`), if one could be fetched.
    pub thumbnail: Option<String>,
}

/// Current state of a recipe's download, as shown in the UI.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct VideoJob {
    pub recipe_id: String,
    pub url: String,
    pub stage: VideoStage,
    /// 0–100 within the current stage, when known.
    pub percent: Option<f64>,
    /// Plain-language progress line, e.g. "12.3 MB of 80.1 MB · 2.1 MB/s · 0:35 left".
    pub detail: Option<String>,
    pub error: Option<String>,
    pub info: Option<VideoInfo>,
}

struct Job {
    state: VideoJob,
    cancel: watch::Sender<bool>,
    last_emit: Option<Instant>,
}

#[derive(Default)]
pub struct Videos {
    jobs: Mutex<HashMap<String, Job>>,
}

impl Videos {
    fn jobs(&self) -> std::sync::MutexGuard<'_, HashMap<String, Job>> {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self, recipe_id: &str) -> Option<VideoJob> {
        self.jobs().get(recipe_id).map(|j| j.state.clone())
    }

    pub fn is_running(&self, recipe_id: &str) -> bool {
        self.jobs()
            .get(recipe_id)
            .is_some_and(|j| j.state.stage.is_running())
    }

    /// Whether any download is running (a backup restore waits for none).
    pub fn any_running(&self) -> bool {
        self.jobs().values().any(|j| j.state.stage.is_running())
    }

    /// Asks a running job to stop; it reports `Cancelled` once its process is gone.
    pub fn cancel(&self, recipe_id: &str) {
        if let Some(job) = self.jobs().get(recipe_id) {
            let _ = job.cancel.send(true);
        }
    }

    /// Forgets a finished job (e.g. after its error was dismissed).
    pub fn dismiss(&self, recipe_id: &str) {
        let mut jobs = self.jobs();
        if jobs
            .get(recipe_id)
            .is_some_and(|j| !j.state.stage.is_running())
        {
            jobs.remove(recipe_id);
        }
    }

    /// Applies `change` to the job's state and sends it to the UI (throttled, except
    /// for stage changes).
    fn update(&self, app: &AppHandle, recipe_id: &str, change: impl FnOnce(&mut VideoJob)) {
        let mut jobs = self.jobs();
        let Some(job) = jobs.get_mut(recipe_id) else {
            return;
        };
        let before = job.state.stage;
        change(&mut job.state);
        let due = job.last_emit.is_none_or(|t| t.elapsed() >= EMIT_INTERVAL);
        if before != job.state.stage || due || !job.state.stage.is_running() {
            job.last_emit = Some(Instant::now());
            let _ = app.emit(EVENT, &job.state);
        }
    }
}

/// Starts downloading `url` for the recipe in the background. The recipe doesn't need
/// to be saved yet; the finished file is linked when it is.
pub fn start(app: &AppHandle, recipe_id: &str, url: &str) -> AppResult<VideoJob> {
    let url = youtube_url(url).ok_or_else(|| {
        AppError::Validation("That doesn't look like a YouTube video link.".into())
    })?;
    let state = app.state::<AppState>();
    let (cancel, cancel_rx) = watch::channel(false);
    let job = VideoJob {
        recipe_id: recipe_id.to_string(),
        url: url.clone(),
        stage: VideoStage::Preparing,
        percent: None,
        detail: None,
        error: None,
        info: None,
    };
    {
        let mut jobs = state.videos.jobs();
        if jobs
            .get(recipe_id)
            .is_some_and(|j| j.state.stage.is_running())
        {
            return Err(AppError::Validation(
                "This recipe's video is already downloading.".into(),
            ));
        }
        jobs.insert(
            recipe_id.to_string(),
            Job {
                state: job.clone(),
                cancel,
                last_emit: None,
            },
        );
    }
    let (app, id) = (app.clone(), recipe_id.to_string());
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let result = run(&app, &state, &id, &url, cancel_rx).await;
        state.media.remove_video_temp(&id);
        match result {
            Ok(()) => {}
            Err(AppError::Cancelled) => {
                log::info!("video download for {id} cancelled");
                state.videos.update(&app, &id, |j| {
                    j.stage = VideoStage::Cancelled;
                    j.percent = None;
                    j.detail = None;
                });
            }
            Err(e) => {
                log::error!("video download for {id} failed: {e:?}");
                set_failed(&app, &state, &id, e.user_message());
            }
        }
    });
    Ok(job)
}

fn set_failed(app: &AppHandle, state: &AppState, id: &str, message: String) {
    // Only a recipe without a working video is marked failed; a failed replacement
    // leaves the old video in place.
    let conn = state.db();
    if let Ok(Some(media)) = recipes::media_of(&conn, id) {
        let has_file = media
            .video_path
            .as_deref()
            .is_some_and(|p| state.media.resolve(p).is_some());
        if !has_file && let Err(e) = recipes::set_video(&conn, id, None, VideoStatus::Failed, None)
        {
            log::error!("could not record failed download for {id}: {e:?}");
        }
    }
    drop(conn);
    state.videos.update(app, id, |j| {
        j.stage = VideoStage::Failed;
        j.percent = None;
        j.detail = None;
        j.error = Some(message);
    });
}

async fn run(
    app: &AppHandle,
    state: &AppState,
    id: &str,
    url: &str,
    mut cancel: watch::Receiver<bool>,
) -> AppResult<()> {
    let set = |change: &dyn Fn(&mut VideoJob)| state.videos.update(app, id, change);

    if !state.tools.all_installed() {
        set(&|j| j.stage = VideoStage::InstallingTools);
        let progress = |p: crate::tools::ToolProgress| {
            state.videos.update(app, id, |j| {
                j.percent = p.total.map(|t| percent(p.downloaded, t));
                j.detail = Some(format!(
                    "Downloading {} · {}",
                    p.name,
                    size_progress(p.downloaded, p.total)
                ));
            });
        };
        tokio::select! {
            r = state.tools.install(false, &progress) => { r?; }
            () = process::cancelled(&mut cancel) => return Err(AppError::Cancelled),
        }
    }

    // Metadata and format choice.
    set(&|j| {
        j.stage = VideoStage::Preparing;
        j.percent = None;
        j.detail = Some("Reading video details…".into());
    });
    let mut cmd = ytdlp(state);
    cmd.args(["-J", "-f", FORMAT, "-S", SORT, "--", url]);
    let mut json = String::new();
    let done = process::run(cmd, &mut cancel, |line| {
        json.push_str(line);
        json.push('\n');
    })
    .await?;
    if !done.status.success() {
        return Err(ytdlp_error(&done.stderr));
    }
    let plan = plan_download(&json)?;

    let thumbnail = match &plan.thumbnail {
        Some(url) => fetch_thumbnail(state, url).await,
        None => None,
    };
    let info = VideoInfo {
        title: plan.title.clone(),
        duration_seconds: plan.duration,
        thumbnail,
    };
    set(&|j| j.info = Some(info.clone()));

    let needed =
        plan.estimated_bytes.unwrap_or(0) * if plan.transcode { 2 } else { 1 } + SPACE_MARGIN;
    if let Ok(free) = fs4::available_space(state.media.tmp_dir())
        && free < needed
    {
        return Err(AppError::Video {
            message: format!(
                "Not enough disk space: the video needs about {} and only {} is free.",
                bytes(needed),
                bytes(free)
            ),
            detail: format!("free {free}, needed {needed}"),
        });
    }

    // Download.
    set(&|j| {
        j.stage = VideoStage::Downloading;
        j.percent = Some(0.0);
        j.detail = None;
    });
    let template = state
        .media
        .tmp_dir()
        .join(format!("{id}.dl.%(ext)s"))
        .to_string_lossy()
        // A literal % in the folder path would be read as a template field.
        .replace('%', "%%")
        .replace("%%(ext)s", "%(ext)s");
    let mut cmd = ytdlp(state);
    cmd.args(["-f", &plan.format_id, "-o", &template])
        .args([
            "--merge-output-format",
            if plan.transcode { "mkv" } else { "mp4" },
        ])
        .args(["--no-simulate", "--progress", "--newline"])
        .args([
            "--progress-template",
            "download:RECIPIO_PROGRESS %(progress)j",
        ])
        .args(["--print", "after_move:RECIPIO_FILE %(filepath)s"])
        .args(["--", url]);
    let mut tracker = PartTracker::new(plan.part_sizes.clone());
    let mut output: Option<PathBuf> = None;
    let done = process::run(cmd, &mut cancel, |line| {
        if let Some(json) = line.strip_prefix("RECIPIO_PROGRESS ") {
            if let Some(p) = parse_progress(json) {
                let (pct, detail) = tracker.update(&p);
                set(&|j| {
                    j.percent = Some(pct);
                    j.detail = Some(detail.clone());
                });
            }
        } else if let Some(path) = line.strip_prefix("RECIPIO_FILE ") {
            output = Some(PathBuf::from(path));
        }
    })
    .await?;
    if !done.status.success() {
        return Err(ytdlp_error(&done.stderr));
    }
    let downloaded = output
        .filter(|p| p.is_file())
        .or_else(|| state.media.find_video_temp(id))
        .ok_or_else(|| AppError::Video {
            message: "The download finished but the video file is missing. Please try again."
                .into(),
            detail: done.stderr.join("\n"),
        })?;

    let mp4 = if plan.transcode {
        set(&|j| {
            j.stage = VideoStage::Transcoding;
            j.percent = plan.duration.map(|_| 0.0);
            j.detail = Some("This video needs converting, which may take a while.".into());
        });
        let out = state.media.tmp_dir().join(format!("{id}.transcode.mp4"));
        let on_percent = |pct: f64| set(&|j| j.percent = Some(pct));
        transcode(
            state,
            &downloaded,
            &out,
            plan.duration,
            &mut cancel,
            &on_percent,
        )
        .await?;
        out
    } else {
        downloaded
    };

    let rel = state.media.commit_video(id, &mp4)?;
    {
        let conn = state.db();
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        // No row yet means the recipe hasn't been saved; `save_recipe` links the file.
        recipes::set_video(&conn, id, Some(&rel), VideoStatus::Ready, Some((url, &now)))?;
    }
    log::info!("video for {id} saved as {rel}");
    set(&|j| {
        j.stage = VideoStage::Done;
        j.percent = Some(100.0);
        j.detail = None;
    });
    Ok(())
}

/// yt-dlp with the options every call shares.
fn ytdlp(state: &AppState) -> tokio::process::Command {
    let tools = &state.tools;
    let mut cmd = process::command(&tools.path("yt-dlp"));
    let deno = format!("deno:{}", tools.path("deno").display());
    cmd.arg("--ignore-config")
        .arg("--no-playlist")
        .args(["--js-runtimes", &deno])
        .arg("--ffmpeg-location")
        .arg(tools.dir())
        .arg("--cache-dir")
        .arg(tools.dir().join("cache"))
        .args(["--encoding", "utf-8"]);
    cmd
}

async fn transcode(
    state: &AppState,
    input: &Path,
    output: &Path,
    duration: Option<f64>,
    cancel: &mut watch::Receiver<bool>,
    on_percent: &(dyn Fn(f64) + Sync),
) -> AppResult<()> {
    let mut cmd = process::command(&state.tools.path("ffmpeg"));
    cmd.args(["-hide_banner", "-nostats", "-y", "-i"])
        .arg(input)
        .args(["-map", "0:v:0", "-map", "0:a:0?"])
        .args([
            "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p",
        ])
        .args(["-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart"])
        .args(["-progress", "pipe:1"])
        .arg(output);
    let done = process::run(cmd, cancel, |line| {
        if let (Some(total), Some(us)) = (duration, line.strip_prefix("out_time_us="))
            && let Ok(us) = us.trim().parse::<f64>()
        {
            on_percent((us / 1e6 / total * 100.0).clamp(0.0, 99.9));
        }
    })
    .await?;
    if !done.status.success() {
        return Err(AppError::Video {
            message: "Converting the video failed. Details are in the log file.".into(),
            detail: done.stderr.join("\n"),
        });
    }
    Ok(())
}

/// Downloads the thumbnail into the staging area. Failures only mean no photo.
async fn fetch_thumbnail(state: &AppState, url: &str) -> Option<String> {
    if !is_youtube_image(url) {
        log::warn!("ignoring thumbnail from unexpected host: {url}");
        return None;
    }
    let result: AppResult<String> = async {
        let resp = state
            .tools
            .client()
            .get(url)
            .send()
            .await?
            .error_for_status()?;
        let ext = match resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
        {
            Some(t) if t.starts_with("image/jpeg") => "jpg",
            Some(t) if t.starts_with("image/webp") => "webp",
            Some(t) if t.starts_with("image/png") => "png",
            other => return Err(AppError::Download(format!("thumbnail type {other:?}"))),
        };
        if resp
            .content_length()
            .is_some_and(|n| n as usize > MAX_THUMBNAIL_BYTES)
        {
            return Err(AppError::Download("thumbnail too large".into()));
        }
        let data = resp.bytes().await?;
        if data.len() > MAX_THUMBNAIL_BYTES {
            return Err(AppError::Download("thumbnail too large".into()));
        }
        state.media.stage_bytes(ext, &data)
    }
    .await;
    result
        .inspect_err(|e| log::warn!("thumbnail download failed: {e:?}"))
        .ok()
}

/// Accepts YouTube video links and returns them cleaned up; `None` for anything else.
pub fn youtube_url(text: &str) -> Option<String> {
    let text = text.trim();
    if text.len() > 2048 {
        return None;
    }
    let url = reqwest::Url::parse(text).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let ok_scheme = matches!(url.scheme(), "https" | "http");
    let ok_host = matches!(
        host.as_str(),
        "youtube.com"
            | "www.youtube.com"
            | "m.youtube.com"
            | "music.youtube.com"
            | "youtu.be"
            | "www.youtube-nocookie.com"
    );
    let has_video = match host.as_str() {
        "youtu.be" => url.path().len() > 1,
        _ => {
            url.query_pairs().any(|(k, v)| k == "v" && !v.is_empty())
                || ["/shorts/", "/embed/", "/live/"]
                    .iter()
                    .any(|p| url.path().starts_with(p) && url.path().len() > p.len())
        }
    };
    (ok_scheme && ok_host && has_video && url.username().is_empty() && url.password().is_none())
        .then(|| url.to_string())
}

/// The video id of a YouTube link (`v=`, youtu.be path, `/shorts/…` etc.), to tell
/// whether two links show the same video.
pub fn youtube_id(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(&youtube_url(url)?).ok()?;
    let id = if url.host_str()? == "youtu.be" {
        url.path().trim_start_matches('/').to_string()
    } else if let Some((_, v)) = url.query_pairs().find(|(k, _)| k == "v") {
        v.into_owned()
    } else {
        url.path_segments()?.nth(1)?.to_string()
    };
    let id = id.split('/').next().unwrap_or_default().to_string();
    (!id.is_empty()).then_some(id)
}

fn is_youtube_image(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some_and(|h| {
                ["ytimg.com", "ggpht.com", "googleusercontent.com"]
                    .iter()
                    .any(|d| h == *d || h.ends_with(&format!(".{d}")))
            })
    })
}

/// What `yt-dlp -J` chose and what we need to know before downloading.
#[derive(Debug, Clone, PartialEq)]
struct Plan {
    title: String,
    duration: Option<f64>,
    thumbnail: Option<String>,
    /// e.g. "137+140".
    format_id: String,
    transcode: bool,
    estimated_bytes: Option<u64>,
    /// Size of each downloaded part (video, audio), for overall progress.
    part_sizes: Vec<Option<u64>>,
}

fn plan_download(json: &str) -> AppResult<Plan> {
    let bad = |what: &str| AppError::Video {
        message:
            "YouTube returned something unexpected. Updating the downloader in Settings may help."
                .into(),
        detail: format!("yt-dlp -J: {what}"),
    };
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| bad(&e.to_string()))?;
    let str_of =
        |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let format_id = str_of(&v, "format_id").ok_or_else(|| bad("no format_id"))?;
    // Video-only + audio-only pair, or one combined format.
    let parts: Vec<&serde_json::Value> = match v.get("requested_formats").and_then(|f| f.as_array())
    {
        Some(list) if !list.is_empty() => list.iter().collect(),
        _ => vec![&v],
    };
    let codec = |kind: &str| {
        parts
            .iter()
            .filter_map(|p| str_of(p, kind))
            .find(|c| c != "none")
            .unwrap_or_default()
    };
    let (vcodec, acodec) = (codec("vcodec"), codec("acodec"));
    let transcode =
        !vcodec.starts_with("avc1") || !(acodec.is_empty() || acodec.starts_with("mp4a"));

    let short_side = parts
        .iter()
        .filter_map(|p| {
            let w = p.get("width")?.as_u64()?;
            let h = p.get("height")?.as_u64()?;
            Some(w.min(h))
        })
        .max();
    if short_side.is_some_and(|s| s > MAX_SHORT_SIDE) {
        return Err(AppError::Video {
            message: "This video isn't available at 1080p or lower.".into(),
            detail: format!("chosen format {format_id} has short side {short_side:?}"),
        });
    }

    let size = |p: &serde_json::Value| {
        ["filesize", "filesize_approx"]
            .iter()
            .find_map(|k| p.get(*k).and_then(|x| x.as_f64()))
            .map(|n| n as u64)
    };
    let part_sizes: Vec<Option<u64>> = parts.iter().map(|p| size(p)).collect();
    let estimated_bytes = part_sizes.iter().copied().sum::<Option<u64>>();
    Ok(Plan {
        title: str_of(&v, "title").unwrap_or_default(),
        duration: v.get("duration").and_then(|d| d.as_f64()),
        thumbnail: str_of(&v, "thumbnail"),
        format_id,
        transcode,
        estimated_bytes,
        part_sizes,
    })
}

#[derive(Debug, Clone, PartialEq, Default)]
struct Progress {
    filename: String,
    downloaded: u64,
    total: Option<u64>,
    speed: Option<f64>,
    eta: Option<u64>,
    finished: bool,
}

fn parse_progress(json: &str) -> Option<Progress> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let num = |k: &str| v.get(k).and_then(|x| x.as_f64());
    Some(Progress {
        filename: v
            .get("filename")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        downloaded: num("downloaded_bytes").unwrap_or(0.0) as u64,
        total: num("total_bytes")
            .or(num("total_bytes_estimate"))
            .map(|n| n as u64),
        speed: num("speed"),
        eta: num("eta").map(|n| n as u64),
        finished: v.get("status").and_then(|x| x.as_str()) == Some("finished"),
    })
}

/// Turns per-file progress (yt-dlp downloads video and audio separately) into one
/// overall percentage, weighting parts by size when known.
struct PartTracker {
    sizes: Vec<Option<u64>>,
    files: Vec<String>,
}

impl PartTracker {
    fn new(sizes: Vec<Option<u64>>) -> Self {
        Self {
            sizes,
            files: Vec::new(),
        }
    }

    fn update(&mut self, p: &Progress) -> (f64, String) {
        let index = match self.files.iter().position(|f| *f == p.filename) {
            Some(i) => i,
            None => {
                self.files.push(p.filename.clone());
                self.files.len() - 1
            }
        };
        let parts = self.sizes.len().max(index + 1);
        let frac = match p.total {
            Some(t) if t > 0 => (p.downloaded as f64 / t as f64).min(1.0),
            _ if p.finished => 1.0,
            _ => 0.0,
        };
        let known: Option<Vec<u64>> = self.sizes.iter().copied().collect();
        let overall = match known.filter(|s| s.len() == parts && s.iter().sum::<u64>() > 0) {
            Some(sizes) => {
                let total: u64 = sizes.iter().sum();
                let before: u64 = sizes[..index].iter().sum();
                (before as f64 + frac * sizes[index] as f64) / total as f64
            }
            None => (index as f64 + frac) / parts as f64,
        };
        let mut detail = size_progress(p.downloaded, p.total);
        if let Some(s) = p.speed.filter(|s| *s > 0.0) {
            detail.push_str(&format!(" · {}/s", bytes(s as u64)));
        }
        if let Some(eta) = p.eta.filter(|_| !p.finished) {
            detail.push_str(&format!(" · {} left", duration::clock(eta)));
        }
        if parts > 1 {
            let what = if index == 0 { "video" } else { "audio" };
            detail = format!("Part {} of {parts} ({what}) · {detail}", index + 1);
        }
        ((overall * 100.0).clamp(0.0, 100.0), detail)
    }
}

/// Maps yt-dlp's error output to a plain-language message; the full text goes to the log.
fn ytdlp_error(stderr: &[String]) -> AppError {
    let text = stderr.join("\n");
    AppError::Video {
        message: classify(&text).into(),
        detail: text,
    }
}

fn classify(stderr: &str) -> &'static str {
    let s = stderr.to_lowercase();
    let any = |needles: &[&str]| needles.iter().any(|n| s.contains(n));
    if any(&["no space left", "errno 28", "disk full"]) {
        "The disk is full. Free up some space and try again."
    } else if any(&["confirm you're not a bot", "confirm you’re not a bot"]) {
        "YouTube is asking for a sign-in to prove this isn't a bot. Try again later, or update the downloader in Settings."
    } else if any(&[
        "sign in to confirm your age",
        "age-restricted",
        "inappropriate for some users",
    ]) {
        "This video is age-restricted and can't be downloaded without signing in."
    } else if any(&["private video", "members-only", "join this channel"]) {
        "This video is private or members-only."
    } else if any(&[
        "video unavailable",
        "has been removed",
        "no longer available",
        "account associated with this video has been terminated",
        "not available in your country",
        "this video is not available",
    ]) {
        "This video is unavailable — it may have been removed or blocked in your country."
    } else if any(&[
        "unsupported url",
        "is not a valid url",
        "incomplete youtube id",
    ]) {
        "That link isn't a YouTube video Recipio can download."
    } else if any(&["requested format is not available"]) {
        "YouTube didn't offer a downloadable format for this video. Updating the downloader in Settings may help."
    } else if any(&[
        "unable to download webpage",
        "unable to download api page",
        "getaddrinfo",
        "name resolution",
        "nodename nor servname",
        "timed out",
        "connection reset",
        "connection refused",
        "network is unreachable",
        "http error 5",
        "unable to connect",
    ]) {
        "Couldn't reach YouTube. Check your internet connection and try again."
    } else {
        "The download failed. Updating the downloader in Settings may help; details are in the log file."
    }
}

fn percent(done: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (done as f64 / total as f64 * 100.0).min(100.0)
    }
}

fn size_progress(done: u64, total: Option<u64>) -> String {
    match total {
        Some(t) => format!("{} of {}", bytes(done), bytes(t)),
        None => bytes(done),
    }
}

fn bytes(n: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let n = n as f64;
    if n >= 1024.0 * MB {
        format!("{:.1} GB", n / 1024.0 / MB)
    } else if n >= MB {
        format!("{:.1} MB", n / MB)
    } else {
        format!("{:.0} KB", n / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_youtube_links_only() {
        for ok in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "  https://youtube.com/watch?v=dQw4w9WgXcQ&list=PL123  ",
            "http://m.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?t=42",
            "https://www.youtube.com/shorts/abc123",
            "https://music.youtube.com/watch?v=abc",
        ] {
            assert!(youtube_url(ok).is_some(), "{ok}");
        }
        for bad in [
            "",
            "dQw4w9WgXcQ",
            "https://www.youtube.com/",
            "https://www.youtube.com/watch?v=",
            "https://youtu.be/",
            "https://www.youtube.com.evil.example/watch?v=abc",
            "https://evil.example/?u=https://www.youtube.com/watch?v=abc",
            "ftp://www.youtube.com/watch?v=abc",
            "file:///etc/passwd",
            "https://user:pw@www.youtube.com/watch?v=abc",
            "--exec=rm -rf ~",
        ] {
            assert_eq!(youtube_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn extracts_video_ids() {
        for url in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=30",
            "https://youtu.be/dQw4w9WgXcQ?si=abc",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube.com/embed/dQw4w9WgXcQ?feature=oembed",
            "https://m.youtube.com/watch?feature=share&v=dQw4w9WgXcQ",
        ] {
            assert_eq!(youtube_id(url).as_deref(), Some("dQw4w9WgXcQ"), "{url}");
        }
        assert_eq!(youtube_id("https://example.com/watch?v=x"), None);
    }

    #[test]
    fn checks_thumbnail_hosts() {
        assert!(is_youtube_image(
            "https://i.ytimg.com/vi/abc/maxresdefault.jpg"
        ));
        assert!(is_youtube_image("https://yt3.ggpht.com/x"));
        assert!(!is_youtube_image("http://i.ytimg.com/vi/abc/1.jpg"));
        assert!(!is_youtube_image("https://ytimg.com.evil.example/1.jpg"));
        assert!(!is_youtube_image("https://evilytimg.com/1.jpg"));
    }

    #[test]
    fn plans_direct_mp4_downloads() {
        let json = r#"{"title":"Best Pancakes","duration":613.0,"thumbnail":"https://i.ytimg.com/vi/x/maxresdefault.webp",
            "format_id":"137+140","requested_formats":[
              {"format_id":"137","vcodec":"avc1.640028","acodec":"none","width":1920,"height":1080,"filesize":80000000},
              {"format_id":"140","vcodec":"none","acodec":"mp4a.40.2","filesize_approx":10000000}]}"#;
        let plan = plan_download(json).unwrap();
        assert_eq!(plan.title, "Best Pancakes");
        assert_eq!(plan.format_id, "137+140");
        assert!(!plan.transcode);
        assert_eq!(plan.estimated_bytes, Some(90_000_000));
        assert_eq!(plan.duration, Some(613.0));
    }

    #[test]
    fn plans_transcodes_and_vertical_videos() {
        let vp9 = r#"{"title":"t","format_id":"248+251","requested_formats":[
            {"vcodec":"vp9","acodec":"none","width":1920,"height":1080},
            {"vcodec":"none","acodec":"opus"}]}"#;
        let plan = plan_download(vp9).unwrap();
        assert!(plan.transcode);
        assert_eq!(plan.estimated_bytes, None);

        let short = r#"{"title":"t","format_id":"18","vcodec":"avc1.42001E","acodec":"mp4a.40.2","width":1080,"height":1920}"#;
        let plan = plan_download(short).unwrap();
        assert!(!plan.transcode);
        assert_eq!(plan.part_sizes, vec![None]);

        let too_big = r#"{"title":"t","format_id":"401","vcodec":"av01","acodec":"none","width":3840,"height":2160}"#;
        assert!(plan_download(too_big).is_err());
        assert!(plan_download("not json").is_err());
    }

    #[test]
    fn tracks_progress_across_parts() {
        let mut t = PartTracker::new(vec![Some(900), Some(100)]);
        let p = |file: &str, done: u64, total: u64| Progress {
            filename: file.into(),
            downloaded: done,
            total: Some(total),
            speed: Some(2.0 * 1024.0 * 1024.0),
            eta: Some(75),
            finished: false,
        };
        let (pct, detail) = t.update(&p("v.mp4", 450, 900));
        assert!((pct - 45.0).abs() < 0.01);
        assert!(detail.starts_with("Part 1 of 2 (video)"), "{detail}");
        assert!(
            detail.contains("2.0 MB/s") && detail.contains("1:15 left"),
            "{detail}"
        );
        let (pct, _) = t.update(&p("a.m4a", 50, 100));
        assert!((pct - 95.0).abs() < 0.01);

        let mut unknown = PartTracker::new(vec![None]);
        let (pct, detail) = unknown.update(&p("f.mp4", 1, 4));
        assert!((pct - 25.0).abs() < 0.01);
        assert!(!detail.starts_with("Part"));
    }

    #[test]
    fn parses_progress_json() {
        let line = r#"{"status":"downloading","downloaded_bytes":1048576,"total_bytes":null,"total_bytes_estimate":4194304.5,"speed":null,"eta":3,"filename":"/tmp/x.f137.mp4"}"#;
        let p = parse_progress(line).unwrap();
        assert_eq!(p.downloaded, 1_048_576);
        assert_eq!(p.total, Some(4_194_304));
        assert_eq!(p.speed, None);
        assert_eq!(p.eta, Some(3));
        assert!(!p.finished);
        assert_eq!(parse_progress("NA"), None);
    }

    #[test]
    fn classifies_errors() {
        let cases = [
            (
                "ERROR: [youtube] abc: Private video. Sign in if you've been granted access",
                "private",
            ),
            (
                "ERROR: [youtube] abc: Video unavailable. This video has been removed by the uploader",
                "unavailable",
            ),
            (
                "ERROR: [youtube] abc: Sign in to confirm your age.",
                "age-restricted",
            ),
            (
                "ERROR: [youtube] abc: Sign in to confirm you’re not a bot.",
                "bot",
            ),
            (
                "ERROR: Unable to download webpage: <urlopen error [Errno 8] nodename nor servname provided>",
                "internet",
            ),
            (
                "ERROR: unable to write data: [Errno 28] No space left on device",
                "disk is full",
            ),
            (
                "ERROR: [generic] Unsupported URL: https://x",
                "isn't a YouTube video",
            ),
            ("ERROR: something new", "log file"),
        ];
        for (stderr, expect) in cases {
            assert!(
                classify(stderr).contains(expect),
                "{stderr} → {}",
                classify(stderr)
            );
        }
    }

    #[test]
    fn formats_sizes_and_times() {
        assert_eq!(bytes(512 * 1024), "512 KB");
        assert_eq!(bytes(12_900_000), "12.3 MB");
        assert_eq!(bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
        assert_eq!(duration::clock(75), "1:15");
        assert_eq!(duration::clock(3725), "1:02:05");
    }
}
