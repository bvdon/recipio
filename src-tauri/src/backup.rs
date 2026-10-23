//! Library backup: one zip holding `recipio-backup.json` (every recipe and label) plus the
//! photo and video files under `media/`. Restoring replaces the whole library (PLAN.md Q5).
//!
//! Sharing uses the same zip layout for chosen recipes (`.recipio` files, format
//! `recipio-share`, no personal ratings). Adding shared recipes only ever adds: recipes
//! already in the library come in as copies with new ids.
//!
//! The JSON is its own versioned format (not a table dump), so backups stay readable
//! after schema migrations.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::AppState;
use crate::db::{bookmarks, labels, recipes};
use crate::error::{AppError, AppResult};
use crate::media::PHOTO_EXTENSIONS;
use crate::models::{
    IngredientLine, LabelKind, Nutrition, PhotoChange, Recipe, RecipeInput, VideoStatus,
};
use crate::quantity;

pub const EVENT: &str = "backup://progress";
const MANIFEST: &str = "recipio-backup.json";
const FORMAT: &str = "recipio-backup";
const SHARE_FORMAT: &str = "recipio-share";
const VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: u64 = 200 * 1024 * 1024;
/// Free space kept in reserve when unpacking.
const SPARE_BYTES: u64 = 200 * 1024 * 1024;
const CHUNK: usize = 1024 * 1024;

/// What a backup of the current library would contain (shown before exporting).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LibraryStats {
    pub recipes: u32,
    pub photos: u32,
    pub videos: u32,
    #[ts(type = "number")]
    pub photo_bytes: u64,
    #[ts(type = "number")]
    pub video_bytes: u64,
}

/// A backup file's contents (shown before restoring it).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct BackupSummary {
    pub created_at: String,
    pub app_version: String,
    pub recipes: u32,
    pub photos: u32,
    pub videos: u32,
}

/// One recipe in a share (or backup) file, for choosing what to add.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct SharedRecipe {
    pub id: String,
    pub title: String,
    pub has_photo: bool,
    pub has_video: bool,
    /// A recipe with this id is already in the library (it would be added as a copy).
    pub in_library: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct SharePreview {
    pub created_at: String,
    pub recipes: Vec<SharedRecipe>,
}

/// Backup = whole library, restored by replacing; share = chosen recipes, added.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Backup,
    Share,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct BackupProgress {
    #[ts(type = "number")]
    pub done: u64,
    #[ts(type = "number")]
    pub total: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    #[serde(default)]
    app_version: String,
    #[serde(default)]
    created_at: String,
    /// All labels, including unused ones.
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    recipes: Vec<BackupRecipe>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BackupRecipe {
    id: String,
    title: String,
    #[serde(default)]
    description: String,
    servings: Option<u32>,
    prep_minutes: Option<u32>,
    cook_minutes: Option<u32>,
    #[serde(default)]
    notes: String,
    rating: Option<u8>,
    youtube_url: Option<String>,
    source_url: Option<String>,
    #[serde(default)]
    nutrition: Nutrition,
    #[serde(default)]
    ingredients: Vec<BackupIngredient>,
    #[serde(default)]
    steps: Vec<String>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    bookmarks: Vec<BackupBookmark>,
    /// Status in the exported library; a video left out of the backup restores as
    /// `missing` so Re-download is offered.
    video_status: VideoStatus,
    /// Zip entry names, `media/photos/<id>.<ext>` and `media/videos/<id>.mp4`.
    photo: Option<String>,
    video: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BackupBookmark {
    seconds: u32,
    #[serde(default)]
    label: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct BackupIngredient {
    #[serde(default)]
    is_heading: bool,
    quantity: Option<f64>,
    quantity_max: Option<f64>,
    unit: Option<String>,
    item: String,
    prep_note: Option<String>,
}

/// One export or restore at a time, with cancel for exports.
#[derive(Default)]
pub struct Backups {
    busy: AtomicBool,
    cancel: AtomicBool,
}

struct Busy<'a>(&'a Backups);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::SeqCst);
    }
}

impl Backups {
    fn start(&self) -> AppResult<Busy<'_>> {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err(AppError::Validation(
                "A backup is already being made or restored.".into(),
            ));
        }
        self.cancel.store(false, Ordering::SeqCst);
        Ok(Busy(self))
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// Reports progress at most every 100 ms (and always at the end).
struct Progress<'a> {
    done: u64,
    total: u64,
    last: Option<Instant>,
    report: &'a dyn Fn(BackupProgress),
}

impl Progress<'_> {
    fn add(&mut self, n: u64) {
        self.done += n;
        if self
            .last
            .is_none_or(|t| t.elapsed() >= Duration::from_millis(100))
            || self.done >= self.total
        {
            self.last = Some(Instant::now());
            (self.report)(BackupProgress {
                done: self.done,
                total: self.total,
            });
        }
    }
}

/// Contents of the library, or of the recipes in `ids`.
pub fn stats(state: &AppState, ids: Option<&[String]>) -> AppResult<LibraryStats> {
    let all = all_recipes(state, ids)?;
    let mut s = LibraryStats {
        recipes: all.len() as u32,
        photos: 0,
        videos: 0,
        photo_bytes: 0,
        video_bytes: 0,
    };
    for r in &all {
        if let Some(n) = r.photo_path.as_deref().and_then(|p| state.media.size_of(p)) {
            s.photos += 1;
            s.photo_bytes += n;
        }
        if let Some(n) = ready_video(r).and_then(|p| state.media.size_of(p)) {
            s.videos += 1;
            s.video_bytes += n;
        }
    }
    Ok(s)
}

/// Every recipe (by title), or only those in `only`.
fn all_recipes(state: &AppState, only: Option<&[String]>) -> AppResult<Vec<Recipe>> {
    let conn = state.db();
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM recipes ORDER BY title COLLATE NOCASE, id")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    ids.iter()
        .filter(|id| only.is_none_or(|o| o.contains(id)))
        .map(|id| recipes::get(&conn, id))
        .collect()
}

fn ready_video(r: &Recipe) -> Option<&str> {
    (r.video_status == VideoStatus::Ready)
        .then_some(r.video_path.as_deref())
        .flatten()
}

/// Writes the library (backup) or the recipes in `ids` (share) to `dest`, via a `.part`
/// file renamed when complete.
pub fn export(
    state: &AppState,
    dest: &Path,
    kind: Kind,
    ids: Option<&[String]>,
    include_videos: bool,
    report: &dyn Fn(BackupProgress),
) -> AppResult<()> {
    let _busy = state.backups.start()?;
    let all = all_recipes(state, ids)?;
    if all.is_empty() && kind == Kind::Share {
        return Err(AppError::Validation(
            "Choose at least one recipe to share.".into(),
        ));
    }
    // A share carries only the labels its recipes use (they're on each recipe).
    let (categories, tags) = if kind == Kind::Share {
        (Vec::new(), Vec::new())
    } else {
        let conn = state.db();
        let names = |kind| -> AppResult<Vec<String>> {
            Ok(labels::list(&conn, kind)?
                .into_iter()
                .map(|l| l.name)
                .collect())
        };
        (names(LabelKind::Category)?, names(LabelKind::Tag)?)
    };

    // (zip entry, file on disk, size) for every media file that exists.
    let mut files: Vec<(String, PathBuf, u64)> = Vec::new();
    let mut add = |rel: Option<&str>| -> Option<String> {
        let rel = rel?;
        let path = state.media.resolve(rel)?;
        let size = fs::metadata(&path).ok()?.len();
        let entry = format!("media/{rel}");
        files.push((entry.clone(), path, size));
        Some(entry)
    };
    let mut entries = Vec::with_capacity(all.len());
    for r in all {
        let photo = add(r.photo_path.as_deref());
        let video = if include_videos {
            add(ready_video(&r))
        } else {
            None
        };
        entries.push(BackupRecipe {
            photo,
            video,
            video_status: r.video_status,
            bookmarks: r
                .bookmarks
                .into_iter()
                .map(|b| BackupBookmark {
                    seconds: b.seconds,
                    label: b.label,
                })
                .collect(),
            ingredients: r
                .ingredients
                .into_iter()
                .map(|l| BackupIngredient {
                    is_heading: l.is_heading,
                    quantity: l.quantity,
                    quantity_max: l.quantity_max,
                    unit: l.unit,
                    item: l.item,
                    prep_note: l.prep_note,
                })
                .collect(),
            id: r.id,
            title: r.title,
            description: r.description,
            servings: r.servings,
            prep_minutes: r.prep_minutes,
            cook_minutes: r.cook_minutes,
            notes: r.notes,
            // Star ratings are personal; a friend's copy starts unrated.
            rating: r.rating.filter(|_| kind == Kind::Backup),
            youtube_url: r.youtube_url,
            source_url: r.source_url,
            nutrition: r.nutrition,
            steps: r.steps,
            categories: r.categories,
            tags: r.tags,
            created_at: r.created_at,
            updated_at: r.updated_at,
        });
    }
    let count = entries.len();
    let now = chrono::Utc::now();
    let manifest = serde_json::to_vec_pretty(&Manifest {
        format: match kind {
            Kind::Backup => FORMAT,
            Kind::Share => SHARE_FORMAT,
        }
        .into(),
        version: VERSION,
        app_version: env!("CARGO_PKG_VERSION").into(),
        created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        categories,
        tags,
        recipes: entries,
    })
    .map_err(|e| AppError::Io(io::Error::other(e)))?;

    let mut progress = Progress {
        done: 0,
        total: manifest.len() as u64 + files.iter().map(|f| f.2).sum::<u64>(),
        last: None,
        report,
    };
    let part = part_path(dest);
    let result = (|| -> AppResult<()> {
        let mut zip = ZipWriter::new(BufWriter::new(File::create(&part)?));
        use chrono::{Datelike, Timelike};
        let l = chrono::Local::now();
        let time = zip::DateTime::from_date_and_time(
            l.year().clamp(1980, 2107) as u16,
            l.month() as u8,
            l.day() as u8,
            l.hour() as u8,
            l.minute() as u8,
            l.second().min(59) as u8,
        )
        .unwrap_or_default();
        let options = SimpleFileOptions::default().last_modified_time(time);
        zip.start_file(
            MANIFEST,
            options.compression_method(CompressionMethod::Deflated),
        )
        .map_err(zip_error)?;
        zip.write_all(&manifest)?;
        progress.add(manifest.len() as u64);
        // Photos and videos are already compressed: store them as they are.
        let stored = options.compression_method(CompressionMethod::Stored);
        let mut buf = vec![0; CHUNK];
        for (entry, path, size) in &files {
            zip.start_file(entry, stored.large_file(*size >= u64::from(u32::MAX)))
                .map_err(zip_error)?;
            let mut src = File::open(path)?;
            loop {
                if state.backups.cancelled() {
                    return Err(AppError::Cancelled);
                }
                let n = src.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                zip.write_all(&buf[..n])?;
                progress.add(n as u64);
            }
        }
        let mut out = zip.finish().map_err(zip_error)?;
        out.flush()?;
        out.get_ref().sync_all()?;
        drop(out);
        fs::rename(&part, dest)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&part);
    } else {
        log::info!(
            "exported {count} recipes and {} media files to {}",
            files.len(),
            dest.display()
        );
    }
    result
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

fn zip_error(e: zip::result::ZipError) -> AppError {
    AppError::Io(io::Error::other(e))
}

fn not_a_backup(detail: impl std::fmt::Display) -> AppError {
    log::warn!("not a backup: {detail}");
    AppError::Validation("That file isn’t a Recipio backup.".into())
}

fn not_a_share(detail: impl std::fmt::Display) -> AppError {
    log::warn!("not a share file: {detail}");
    AppError::Validation("That file isn’t a Recipio recipe file.".into())
}

/// Opens a backup (restore) or a share or backup file (adding recipes from it).
fn open(path: &Path) -> AppResult<(ZipArchive<File>, Manifest)> {
    let (zip, m) = open_any(path)?;
    if m.format != FORMAT {
        return Err(not_a_backup("wrong format"));
    }
    Ok((zip, m))
}

fn open_shared(path: &Path) -> AppResult<(ZipArchive<File>, Manifest)> {
    match open_any(path) {
        Err(AppError::Validation(m)) if m.contains("isn’t a Recipio backup") => {
            Err(not_a_share("unreadable"))
        }
        other => other,
    }
}

fn open_any(path: &Path) -> AppResult<(ZipArchive<File>, Manifest)> {
    let mut zip = ZipArchive::new(File::open(path)?).map_err(not_a_backup)?;
    let mut entry = zip.by_name(MANIFEST).map_err(not_a_backup)?;
    if entry.size() > MAX_MANIFEST_BYTES {
        return Err(not_a_backup("manifest too large"));
    }
    let mut json = Vec::new();
    (&mut entry)
        .take(MAX_MANIFEST_BYTES)
        .read_to_end(&mut json)
        .map_err(not_a_backup)?;
    drop(entry);
    let manifest: Manifest = serde_json::from_slice(&json).map_err(not_a_backup)?;
    if manifest.format != FORMAT && manifest.format != SHARE_FORMAT {
        return Err(not_a_backup("wrong format"));
    }
    if manifest.version > VERSION {
        return Err(AppError::Validation(
            "This backup was made by a newer version of Recipio. Please update the app first."
                .into(),
        ));
    }
    Ok((zip, manifest))
}

pub fn inspect(path: &Path) -> AppResult<BackupSummary> {
    let (mut zip, m) = open(path)?;
    let mut present = |name: &Option<String>| name.as_ref().is_some_and(|n| zip.by_name(n).is_ok());
    let (mut photos, mut videos) = (0, 0);
    for r in &m.recipes {
        photos += u32::from(present(&r.photo));
        videos += u32::from(present(&r.video));
    }
    Ok(BackupSummary {
        created_at: m.created_at,
        app_version: m.app_version,
        recipes: m.recipes.len() as u32,
        photos,
        videos,
    })
}

/// A backup recipe checked like a saved one, with its media entry names verified.
struct Restored {
    valid: recipes::ValidRecipe,
    created_at: String,
    updated_at: String,
    video_status: VideoStatus,
    bookmarks: Vec<(u32, String)>,
    /// (zip entry, file extension)
    photo: Option<(String, String)>,
    /// zip entry
    video: Option<String>,
}

impl Restored {
    fn photo_rel(&self) -> Option<String> {
        let id = &self.valid.input.id;
        self.photo
            .as_ref()
            .map(|(_, ext)| format!("photos/{id}.{ext}"))
    }

    fn video_rel(&self) -> Option<String> {
        let id = &self.valid.input.id;
        self.video.as_ref().map(|_| format!("videos/{id}.mp4"))
    }

    /// Video path and status to store: a restored file is ready; a video that was
    /// downloaded but isn't in the file is missing (Re-download is offered).
    fn video_state(&self, kind: Kind) -> (Option<String>, VideoStatus) {
        match (self.video_rel(), self.video_status) {
            (Some(rel), _) => (Some(rel), VideoStatus::Ready),
            (None, _) if kind == Kind::Share => (None, VideoStatus::None),
            (None, VideoStatus::Ready | VideoStatus::Missing) => (None, VideoStatus::Missing),
            (None, VideoStatus::Downloading | VideoStatus::Failed) => (None, VideoStatus::Failed),
            (None, VideoStatus::None) => (None, VideoStatus::None),
        }
    }
}

fn check_recipe(r: BackupRecipe) -> AppResult<Restored> {
    let input = RecipeInput {
        id: r.id.clone(),
        title: r.title.clone(),
        description: r.description,
        servings: r.servings,
        prep_minutes: r.prep_minutes,
        cook_minutes: r.cook_minutes,
        photo: PhotoChange::Keep,
        notes: r.notes,
        rating: r.rating,
        youtube_url: r.youtube_url,
        source_url: r.source_url,
        nutrition: r.nutrition,
        ingredients: Vec::new(),
        steps: r.steps,
        categories: r.categories,
        tags: r.tags,
    };
    let mut valid = recipes::validate(&input).map_err(|e| {
        AppError::Validation(format!(
            "The backup has a recipe Recipio can’t read (“{}”): {}",
            r.title,
            e.user_message()
        ))
    })?;
    let amount = |q: Option<f64>| q.filter(|q| q.is_finite() && *q >= 0.0);
    valid.ingredients = r
        .ingredients
        .into_iter()
        .filter_map(|l| {
            let item = l.item.trim().to_string();
            if item.is_empty() {
                return None;
            }
            let text =
                |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            Some(if l.is_heading {
                IngredientLine {
                    is_heading: true,
                    quantity: None,
                    quantity_max: None,
                    unit: None,
                    item,
                    prep_note: None,
                    quantity_text: String::new(),
                }
            } else {
                let (quantity, quantity_max) = (amount(l.quantity), amount(l.quantity_max));
                IngredientLine {
                    is_heading: false,
                    quantity,
                    quantity_max: quantity_max.filter(|_| quantity.is_some()),
                    unit: text(l.unit),
                    item,
                    prep_note: text(l.prep_note),
                    quantity_text: quantity::format(quantity, quantity_max),
                }
            })
        })
        .collect();

    let id = &valid.input.id;
    // Entry names must be exactly what an export writes; nothing else is unpacked.
    let photo = r.photo.and_then(|entry| {
        let ext = entry.strip_prefix(&format!("media/photos/{id}."))?;
        PHOTO_EXTENSIONS
            .contains(&ext)
            .then(|| (entry.clone(), ext.to_string()))
    });
    let video = r.video.filter(|e| *e == format!("media/videos/{id}.mp4"));
    let timestamp = |t: String| {
        if chrono::DateTime::parse_from_rfc3339(&t).is_ok() {
            t
        } else {
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        }
    };
    Ok(Restored {
        video,
        photo,
        bookmarks: r
            .bookmarks
            .into_iter()
            .map(|b| (b.seconds, b.label))
            .collect(),
        video_status: r.video_status,
        created_at: timestamp(r.created_at),
        updated_at: timestamp(r.updated_at),
        valid,
    })
}

/// Replaces the whole library with the backup at `path`. Everything is checked and
/// unpacked first; the database is replaced in one transaction; then the media folders
/// are swapped. Returns the number of recipes restored.
pub fn restore(state: &AppState, path: &Path, report: &dyn Fn(BackupProgress)) -> AppResult<u32> {
    let _busy = state.backups.start()?;
    if state.videos.any_running() {
        return Err(AppError::Validation(
            "A video is still downloading. Wait for it to finish, or cancel it, then restore."
                .into(),
        ));
    }
    let (mut zip, manifest) = open(path)?;
    let mut seen = HashSet::new();
    let mut restored = Vec::with_capacity(manifest.recipes.len());
    for r in manifest.recipes {
        let r = check_recipe(r)?;
        if !seen.insert(r.valid.input.id.clone()) {
            return Err(not_a_backup("duplicate recipe id"));
        }
        restored.push(r);
    }

    let total = check_entries(&mut zip, &mut restored);
    ensure_space(state, total)?;

    let dir = state.media.begin_restore()?;
    let mut progress = Progress {
        done: 0,
        total,
        last: None,
        report,
    };
    let unpacked = (|| -> AppResult<()> {
        let mut buf = vec![0; CHUNK];
        for r in &restored {
            if let (Some((entry, _)), Some(rel)) = (&r.photo, r.photo_rel()) {
                unpack(&mut zip, entry, &dir.join(rel), &mut buf, &mut progress)?;
            }
            if let (Some(entry), Some(rel)) = (&r.video, r.video_rel()) {
                unpack(&mut zip, entry, &dir.join(rel), &mut buf, &mut progress)?;
            }
        }
        Ok(())
    })();
    if let Err(e) = unpacked {
        state.media.abandon_restore();
        return Err(e);
    }

    let written = (|| -> AppResult<()> {
        let mut conn = state.db();
        let tx = conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM recipes_fts; DELETE FROM recipes; DELETE FROM categories; DELETE FROM tags;",
        )?;
        for (kind, names) in [
            (LabelKind::Category, &manifest.categories),
            (LabelKind::Tag, &manifest.tags),
        ] {
            for name in names {
                if let Err(e) = labels::create(&tx, kind, name) {
                    log::warn!("skipping label {name:?} from backup: {e:?}");
                }
            }
        }
        for r in &restored {
            let photo = r.photo_rel();
            recipes::write(&tx, &r.valid, photo.as_deref(), &r.updated_at)?;
            let (video, status) = r.video_state(Kind::Backup);
            tx.execute(
                "UPDATE recipes SET created_at = ?2, updated_at = ?3, video_path = ?4,
                                    video_status = ?5
                 WHERE id = ?1",
                rusqlite::params![
                    r.valid.input.id,
                    r.created_at,
                    r.updated_at,
                    video,
                    status.as_str()
                ],
            )?;
            bookmarks::insert_all(&tx, &r.valid.input.id, &r.bookmarks)?;
        }
        tx.commit()?;
        Ok(())
    })();
    if let Err(e) = written {
        state.media.abandon_restore();
        return Err(e);
    }
    // The database already points at the new files; if a rename fails, startup marks
    // the affected videos missing and photos simply don't show.
    state.media.finish_restore()?;
    log::info!(
        "restored {} recipes from {}",
        restored.len(),
        path.display()
    );
    Ok(restored.len() as u32)
}

/// Drops media entries the zip doesn't actually have; returns the bytes to unpack (from
/// the zip directory).
fn check_entries(zip: &mut ZipArchive<File>, restored: &mut [Restored]) -> u64 {
    let mut total = 0;
    let mut size = |entry: &str| {
        zip.by_name(entry)
            .map(|f| f.size())
            .inspect_err(|e| log::warn!("entry {entry} missing: {e}"))
            .ok()
    };
    for r in restored {
        match r.photo.as_ref().map(|(entry, _)| size(entry)) {
            Some(Some(n)) => total += n,
            Some(None) => r.photo = None,
            None => {}
        }
        match r.video.as_deref().map(&mut size) {
            Some(Some(n)) => total += n,
            Some(None) => r.video = None,
            None => {}
        }
    }
    total
}

fn ensure_space(state: &AppState, bytes: u64) -> AppResult<()> {
    let free = fs4::available_space(state.media.root())?;
    if free < bytes + SPARE_BYTES {
        return Err(AppError::Validation(format!(
            "There isn’t enough free disk space (this needs about {} MB).",
            (bytes + SPARE_BYTES) / 1_000_000
        )));
    }
    Ok(())
}

/// Copies one zip entry to `dest`, never writing more than the zip declares.
fn unpack(
    zip: &mut ZipArchive<File>,
    entry: &str,
    dest: &Path,
    buf: &mut [u8],
    progress: &mut Progress,
) -> AppResult<()> {
    let mut src = zip.by_name(entry).map_err(zip_error)?;
    let size = src.size();
    let mut out = BufWriter::new(File::create(dest)?);
    let mut written = 0u64;
    loop {
        let n = src.read(buf)?;
        if n == 0 {
            break;
        }
        written += n as u64;
        if written > size {
            return Err(not_a_backup(format!("{entry} is larger than declared")));
        }
        out.write_all(&buf[..n])?;
        progress.add(n as u64);
    }
    out.flush()?;
    Ok(())
}

/// What a share (or backup) file holds, marking recipes already in the library.
pub fn preview(state: &AppState, path: &Path) -> AppResult<SharePreview> {
    let (mut zip, m) = open_shared(path)?;
    let conn = state.db();
    let mut exists = conn.prepare("SELECT 1 FROM recipes WHERE id = ?1")?;
    let mut present = |name: &Option<String>| name.as_ref().is_some_and(|n| zip.by_name(n).is_ok());
    let mut recipes = Vec::with_capacity(m.recipes.len());
    for r in &m.recipes {
        recipes.push(SharedRecipe {
            id: r.id.clone(),
            title: r.title.clone(),
            has_photo: present(&r.photo),
            has_video: present(&r.video),
            in_library: exists.exists([&r.id])?,
        });
    }
    Ok(SharePreview {
        created_at: m.created_at,
        recipes,
    })
}

/// Adds the recipes in `ids` from a share (or backup) file. Nothing in the library is
/// changed or removed; a recipe whose id is taken gets a new id. Returns how many were
/// added.
pub fn add_shared(
    state: &AppState,
    path: &Path,
    ids: &[String],
    report: &dyn Fn(BackupProgress),
) -> AppResult<u32> {
    let _busy = state.backups.start()?;
    let (mut zip, manifest) = open_shared(path)?;
    let mut chosen = Vec::new();
    for r in manifest.recipes.into_iter().filter(|r| ids.contains(&r.id)) {
        chosen.push(check_recipe(r)?);
    }
    if chosen.is_empty() {
        return Err(AppError::Validation(
            "Choose at least one recipe to add.".into(),
        ));
    }
    {
        let conn = state.db();
        let mut exists = conn.prepare("SELECT 1 FROM recipes WHERE id = ?1")?;
        let mut taken = HashSet::new();
        for r in &mut chosen {
            if exists.exists([&r.valid.input.id])? || !taken.insert(r.valid.input.id.clone()) {
                r.valid.input.id = uuid::Uuid::new_v4().to_string();
            }
        }
    }

    let total = check_entries(&mut zip, &mut chosen);
    ensure_space(state, total)?;
    let mut progress = Progress {
        done: 0,
        total,
        last: None,
        report,
    };
    // Unpack into the staging area, then move into place under the (new) ids.
    let mut placed: Vec<String> = Vec::new();
    let moved = (|| -> AppResult<()> {
        let mut buf = vec![0; CHUNK];
        let tmp = state.media.tmp_dir();
        for r in &chosen {
            let id = &r.valid.input.id;
            if let (Some((entry, _)), Some(rel)) = (&r.photo, r.photo_rel()) {
                let ext = rel.rsplit('.').next().unwrap_or_default();
                let staged = format!("tmp/{}.{ext}", uuid::Uuid::new_v4());
                unpack(
                    &mut zip,
                    entry,
                    &state.media.root().join(&staged),
                    &mut buf,
                    &mut progress,
                )?;
                placed.push(state.media.commit_photo(id, &staged)?);
            }
            if let Some(entry) = &r.video {
                let file = tmp.join(format!("{id}.share.mp4"));
                unpack(&mut zip, entry, &file, &mut buf, &mut progress)?;
                placed.push(state.media.commit_video(id, &file)?);
            }
        }
        Ok(())
    })();
    let undo = |placed: &[String]| {
        for rel in placed {
            state.media.remove(rel);
        }
    };
    if let Err(e) = moved {
        undo(&placed);
        return Err(e);
    }

    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let written = (|| -> AppResult<()> {
        let mut conn = state.db();
        let tx = conn.transaction()?;
        for r in &chosen {
            let id = &r.valid.input.id;
            recipes::write(&tx, &r.valid, r.photo_rel().as_deref(), &now)?;
            let (video, status) = r.video_state(Kind::Share);
            recipes::set_video(&tx, id, video.as_deref(), status, None)?;
            bookmarks::insert_all(&tx, id, &r.bookmarks)?;
        }
        tx.commit()?;
        Ok(())
    })();
    if let Err(e) = written {
        undo(&placed);
        return Err(e);
    }
    log::info!(
        "added {} shared recipes from {}",
        chosen.len(),
        path.display()
    );
    Ok(chosen.len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::media::MediaDir;
    use crate::tools::Tools;
    use crate::video::Videos;
    use std::sync::Mutex;

    fn state(dir: &Path) -> AppState {
        AppState {
            db: Mutex::new(open_in_memory()),
            media: MediaDir::new(&dir.join("media")).unwrap(),
            tools: Tools::new(&dir.join("bin")).unwrap(),
            videos: Videos::default(),
            backups: Backups::default(),
            keep_awake: Mutex::new(None),
        }
    }

    fn input(id: &str, title: &str) -> RecipeInput {
        RecipeInput {
            id: id.into(),
            title: title.into(),
            description: "Desc".into(),
            servings: Some(2),
            prep_minutes: Some(5),
            cook_minutes: None,
            photo: PhotoChange::Keep,
            notes: "Notes".into(),
            rating: Some(4),
            youtube_url: Some("https://youtu.be/abc".into()),
            source_url: Some("https://example.com/r".into()),
            nutrition: Nutrition {
                calories: Some(300.0),
                ..Default::default()
            },
            ingredients: vec![
                crate::models::IngredientInput {
                    is_heading: true,
                    quantity: String::new(),
                    unit: None,
                    item: "Dough".into(),
                    prep_note: None,
                },
                crate::models::IngredientInput {
                    is_heading: false,
                    quantity: "1/3".into(),
                    unit: Some("cup".into()),
                    item: "flour".into(),
                    prep_note: Some("sifted".into()),
                },
            ],
            steps: vec!["Mix".into(), "Bake".into()],
            categories: vec!["Baking".into()],
            tags: vec!["easy".into()],
        }
    }

    /// Saves a recipe with a photo and (optionally) a ready video file.
    fn add(state: &AppState, id: &str, title: &str, video: bool) {
        let staged = state
            .media
            .stage_bytes("jpg", b"\xFF\xD8\xFFphoto")
            .unwrap();
        let valid = recipes::validate(&input(id, title)).unwrap();
        let photo = state.media.commit_photo(id, &staged).unwrap();
        let mut conn = state.db();
        recipes::upsert(&mut conn, &valid, Some(&photo), "2026-01-02T03:04:05Z").unwrap();
        if video {
            let rel = format!("videos/{id}.mp4");
            fs::write(state.media.root().join(&rel), b"video bytes").unwrap();
            recipes::set_video(&conn, id, Some(&rel), VideoStatus::Ready, None).unwrap();
        }
    }

    const A: &str = "11111111-1111-4111-8111-111111111111";
    const B: &str = "22222222-2222-4222-8222-222222222222";
    const C: &str = "33333333-3333-4333-8333-333333333333";

    #[test]
    fn exports_and_restores_the_whole_library() {
        let tmp = tempfile::tempdir().unwrap();
        let source = state(&tmp.path().join("one"));
        add(&source, A, "Apple pie", true);
        add(&source, B, "Bread", false);
        labels::create(&source.db(), LabelKind::Tag, "unused").unwrap();

        let stats = stats(&source, None).unwrap();
        assert_eq!((stats.recipes, stats.photos, stats.videos), (2, 2, 1));

        let file = tmp.path().join("backup.zip");
        let reports = Mutex::new(Vec::new());
        export(&source, &file, Kind::Backup, None, true, &|p| {
            reports.lock().unwrap().push(p)
        })
        .unwrap();
        assert!(!part_path(&file).exists());
        let last = reports.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.done, last.total);

        let summary = inspect(&file).unwrap();
        assert_eq!((summary.recipes, summary.photos, summary.videos), (2, 2, 1));

        // A different library that the restore replaces entirely.
        let target = state(&tmp.path().join("two"));
        add(&target, C, "Old soup", true);
        assert_eq!(restore(&target, &file, &|_| {}).unwrap(), 2);

        let conn = target.db();
        assert!(recipes::get(&conn, C).is_err(), "old recipe is gone");
        let a = recipes::get(&conn, A).unwrap();
        let original = recipes::get(&source.db(), A).unwrap();
        assert_eq!(a.title, "Apple pie");
        assert_eq!(a.ingredients, original.ingredients, "exact quantities");
        assert_eq!(a.steps, original.steps);
        assert_eq!(a.categories, vec!["Baking"]);
        assert_eq!(a.tags, vec!["easy"]);
        assert_eq!(a.nutrition, original.nutrition);
        assert_eq!(a.created_at, original.created_at);
        assert_eq!(a.updated_at, original.updated_at);
        assert_eq!(a.video_status, VideoStatus::Ready);
        assert_eq!(target.media.size_of(&a.video_path.unwrap()), Some(11));
        assert!(target.media.resolve(&a.photo_path.unwrap()).is_some());
        assert!(target.media.resolve(&format!("videos/{C}.mp4")).is_none());
        let tags = labels::list(&conn, LabelKind::Tag).unwrap();
        assert!(
            tags.iter().any(|t| t.name == "unused"),
            "unused labels kept"
        );
        let found = recipes::list(
            &conn,
            &crate::models::RecipeQuery {
                search: "bread".into(),
                category_ids: vec![],
                tag_ids: vec![],
                sort: crate::models::RecipeSort::Title,
                descending: false,
            },
        )
        .unwrap();
        assert_eq!(found.len(), 1, "search index rebuilt");
    }

    #[test]
    fn videos_left_out_restore_as_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let source = state(&tmp.path().join("one"));
        add(&source, A, "Apple pie", true);
        let file = tmp.path().join("backup.zip");
        export(&source, &file, Kind::Backup, None, false, &|_| {}).unwrap();
        assert_eq!(inspect(&file).unwrap().videos, 0);

        let target = state(&tmp.path().join("two"));
        restore(&target, &file, &|_| {}).unwrap();
        let a = recipes::get(&target.db(), A).unwrap();
        assert_eq!(a.video_status, VideoStatus::Missing);
        assert_eq!(a.youtube_url.as_deref(), Some("https://youtu.be/abc"));
    }

    fn zip_with(path: &Path, files: &[(&str, &[u8])]) {
        let mut zip = ZipWriter::new(File::create(path).unwrap());
        for (name, data) in files {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn rejects_files_that_are_not_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("x.zip");
        fs::write(&path, b"not a zip").unwrap();
        assert!(
            inspect(&path)
                .unwrap_err()
                .user_message()
                .contains("isn’t a Recipio backup")
        );
        zip_with(&path, &[("other.txt", b"hi")]);
        assert!(inspect(&path).is_err());
        zip_with(
            &path,
            &[(
                MANIFEST,
                br#"{"format":"something-else","version":1,"recipes":[]}"#,
            )],
        );
        assert!(inspect(&path).is_err());
        zip_with(
            &path,
            &[(
                MANIFEST,
                br#"{"format":"recipio-backup","version":99,"recipes":[]}"#,
            )],
        );
        assert!(
            inspect(&path)
                .unwrap_err()
                .user_message()
                .contains("newer version")
        );
    }

    #[test]
    fn never_unpacks_unexpected_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = format!(
            r#"{{"format":"recipio-backup","version":1,"recipes":[{{"id":"{A}","title":"T",
               "servings":null,"prep_minutes":null,"cook_minutes":null,"rating":null,
               "youtube_url":null,"source_url":null,"created_at":"x","updated_at":"x",
               "video_status":"ready","photo":"../../evil.jpg","video":"media/videos/../../evil.mp4"}}]}}"#
        );
        let path = tmp.path().join("evil.zip");
        zip_with(
            &path,
            &[
                (MANIFEST, manifest.as_bytes()),
                ("../../evil.jpg", b"x"),
                ("media/videos/../../evil.mp4", b"x"),
            ],
        );
        let target = state(&tmp.path().join("lib"));
        assert_eq!(restore(&target, &path, &|_| {}).unwrap(), 1);
        let r = recipes::get(&target.db(), A).unwrap();
        assert_eq!(r.photo_path, None);
        assert_eq!(r.video_status, VideoStatus::Missing);
        assert!(!tmp.path().join("evil.jpg").exists());
        assert!(!tmp.path().join("evil.mp4").exists());
    }

    #[test]
    fn a_bad_recipe_leaves_the_library_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = format!(
            r#"{{"format":"recipio-backup","version":1,"recipes":[{{"id":"{A}","title":"   ",
               "servings":null,"prep_minutes":null,"cook_minutes":null,"rating":null,
               "youtube_url":null,"source_url":null,"created_at":"x","updated_at":"x",
               "video_status":"none","photo":null,"video":null}}]}}"#
        );
        let path = tmp.path().join("bad.zip");
        zip_with(&path, &[(MANIFEST, manifest.as_bytes())]);
        let target = state(&tmp.path().join("lib"));
        add(&target, C, "Keep me", false);
        assert!(restore(&target, &path, &|_| {}).is_err());
        assert!(recipes::get(&target.db(), C).is_ok());
    }

    #[test]
    fn shares_chosen_recipes_and_adds_them_as_new() {
        let tmp = tempfile::tempdir().unwrap();
        let mine = state(&tmp.path().join("mine"));
        add(&mine, A, "Apple pie", true);
        add(&mine, B, "Bread", false);
        bookmarks::add(&mine.db(), A, 75, "Crust", None).unwrap();

        let file = tmp.path().join("for friend.recipio");
        let ids = vec![A.to_string()];
        export(&mine, &file, Kind::Share, Some(&ids), false, &|_| {}).unwrap();
        assert!(
            inspect(&file).is_err(),
            "a share file can't be restored over a library"
        );

        // A friend's library already has their own recipe C.
        let friend = state(&tmp.path().join("friend"));
        add(&friend, C, "Soup", false);
        let shown = preview(&friend, &file).unwrap();
        assert_eq!(shown.recipes.len(), 1);
        let p = &shown.recipes[0];
        assert_eq!(
            (p.title.as_str(), p.has_photo, p.has_video, p.in_library),
            ("Apple pie", true, false, false)
        );

        assert_eq!(add_shared(&friend, &file, &ids, &|_| {}).unwrap(), 1);
        let conn = friend.db();
        assert!(recipes::get(&conn, C).is_ok(), "nothing removed");
        let a = recipes::get(&conn, A).unwrap();
        assert_eq!(a.rating, None, "ratings are personal");
        assert_eq!(a.categories, vec!["Baking"]);
        assert_eq!(
            a.video_status,
            VideoStatus::None,
            "can be downloaded from the link"
        );
        assert_eq!(a.youtube_url.as_deref(), Some("https://youtu.be/abc"));
        assert_eq!(a.bookmarks.len(), 1);
        assert_eq!(a.bookmarks[0].time_text, "1:15");
        assert!(
            friend
                .media
                .resolve(&a.photo_path.clone().unwrap())
                .is_some()
        );
        drop(conn);

        // Adding it again makes a copy with a new id; the first stays as it was.
        assert!(preview(&friend, &file).unwrap().recipes[0].in_library);
        add_shared(&friend, &file, &ids, &|_| {}).unwrap();
        let conn = friend.db();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM recipes WHERE title = 'Apple pie'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
        let copy: String = conn
            .query_row(
                "SELECT id FROM recipes WHERE title = 'Apple pie' AND id != ?1",
                [A],
                |r| r.get(0),
            )
            .unwrap();
        let copy = recipes::get(&conn, &copy).unwrap();
        assert!(copy.photo_path.unwrap().contains(&copy.id));
        assert_eq!(copy.bookmarks.len(), 1);
    }

    #[test]
    fn shares_videos_when_asked() {
        let tmp = tempfile::tempdir().unwrap();
        let mine = state(&tmp.path().join("mine"));
        add(&mine, A, "Apple pie", true);
        let file = tmp.path().join("x.recipio");
        export(
            &mine,
            &file,
            Kind::Share,
            Some(&[A.to_string()]),
            true,
            &|_| {},
        )
        .unwrap();
        let friend = state(&tmp.path().join("friend"));
        add_shared(&friend, &file, &[A.to_string()], &|_| {}).unwrap();
        let a = recipes::get(&friend.db(), A).unwrap();
        assert_eq!(a.video_status, VideoStatus::Ready);
        assert_eq!(friend.media.size_of(&a.video_path.unwrap()), Some(11));
    }

    #[test]
    fn restores_bookmarks_and_clears_them_for_a_new_video() {
        let tmp = tempfile::tempdir().unwrap();
        let source = state(&tmp.path().join("one"));
        add(&source, A, "Apple pie", true);
        bookmarks::add(&source.db(), A, 30, "Mix", None).unwrap();
        let file = tmp.path().join("b.zip");
        export(&source, &file, Kind::Backup, None, true, &|_| {}).unwrap();
        let target = state(&tmp.path().join("two"));
        restore(&target, &file, &|_| {}).unwrap();
        assert_eq!(recipes::get(&target.db(), A).unwrap().bookmarks.len(), 1);

        // Same video under another link form: kept. A different video: cleared.
        let conn = target.db();
        let now = "2026-02-02T00:00:00Z";
        recipes::set_video(
            &conn,
            A,
            None,
            VideoStatus::None,
            Some(("https://www.youtube.com/watch?v=abc", now)),
        )
        .unwrap();
        assert_eq!(bookmarks::list(&conn, A).unwrap().len(), 1);
        recipes::set_video(
            &conn,
            A,
            None,
            VideoStatus::None,
            Some(("https://youtu.be/other", now)),
        )
        .unwrap();
        assert!(bookmarks::list(&conn, A).unwrap().is_empty());
    }
}
