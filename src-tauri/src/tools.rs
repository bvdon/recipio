//! The app's own copies of yt-dlp, ffmpeg/ffprobe and Deno (yt-dlp's JavaScript runtime
//! for YouTube), kept in `<app_data>/bin`. Downloaded from the publishers' releases on
//! first use, verified against the publishers' SHA-256 checksums, then made executable.
//! Sources and the trust model are in docs/PLAN.md §3.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::error::{AppError, AppResult};

const MANIFEST: &str = "tools.json";
const TMP: &str = "tmp";
/// No single download we make is anywhere near this; it bounds a misbehaving server.
const MAX_DOWNLOAD_BYTES: u64 = 400 * 1024 * 1024;

/// A downloadable tool. ffmpeg includes ffprobe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum Package {
    YtDlp,
    Ffmpeg,
    Deno,
}

impl Package {
    pub const ALL: [Package; 3] = [Package::YtDlp, Package::Ffmpeg, Package::Deno];

    pub fn label(self) -> &'static str {
        match self {
            Package::YtDlp => "yt-dlp",
            Package::Ffmpeg => "ffmpeg",
            Package::Deno => "Deno",
        }
    }

    /// Executables this package installs into the bin folder (without `.exe`).
    fn executables(self) -> &'static [&'static str] {
        match self {
            Package::YtDlp => &["yt-dlp"],
            Package::Ffmpeg => &["ffmpeg", "ffprobe"],
            Package::Deno => &["deno"],
        }
    }
}

/// What the Settings page shows for one package.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ToolInfo {
    pub package: Package,
    pub name: String,
    /// Installed version, or `None` if not installed.
    pub version: Option<String>,
}

/// Download progress for one package, sent to the UI as it happens.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ToolProgress {
    pub package: Package,
    pub name: String,
    #[ts(type = "number")]
    pub downloaded: u64,
    #[ts(type = "number | null")]
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Installed {
    /// Identifies the exact build (release tag, build folder or checksum), compared
    /// with the latest available to decide whether an update is needed.
    source_id: String,
    version: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Archive {
    /// The download is the executable itself.
    Raw,
    Zip,
    TarXz,
}

#[derive(Debug, Clone, PartialEq)]
enum Checksum {
    /// A `SHA2-256SUMS`-style file listing many assets; look up `name`.
    Manifest { url: String, name: String },
    /// A file holding the checksum of this one asset.
    Single { url: String },
}

/// One file to download and the executables to take from it.
#[derive(Debug, Clone, PartialEq)]
struct Artifact {
    url: String,
    checksum: Checksum,
    archive: Archive,
    executables: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq)]
struct Release {
    source_id: String,
    /// Display version; `None` means "ask the installed binary".
    version: Option<String>,
    artifacts: Vec<Artifact>,
}

/// Where each package comes from on one OS/architecture.
#[derive(Debug, Clone, PartialEq)]
struct Platform {
    yt_dlp_asset: &'static str,
    deno_target: &'static str,
    ffmpeg: FfmpegSource,
}

#[derive(Debug, Clone, PartialEq)]
enum FfmpegSource {
    /// ffmpeg.martin-riedl.de, `arch` is `arm64` or `amd64`.
    MartinRiedl { arch: &'static str },
    /// github.com/yt-dlp/FFmpeg-Builds, rolling `latest` release.
    YtDlpBuilds { asset: &'static str },
}

fn platform(os: &str, arch: &str) -> Option<Platform> {
    let p = match (os, arch) {
        ("macos", "aarch64") => Platform {
            yt_dlp_asset: "yt-dlp_macos",
            deno_target: "aarch64-apple-darwin",
            ffmpeg: FfmpegSource::MartinRiedl { arch: "arm64" },
        },
        ("macos", "x86_64") => Platform {
            yt_dlp_asset: "yt-dlp_macos",
            deno_target: "x86_64-apple-darwin",
            ffmpeg: FfmpegSource::MartinRiedl { arch: "amd64" },
        },
        ("windows", "x86_64") => Platform {
            yt_dlp_asset: "yt-dlp.exe",
            deno_target: "x86_64-pc-windows-msvc",
            ffmpeg: FfmpegSource::YtDlpBuilds {
                asset: "ffmpeg-master-latest-win64-gpl.zip",
            },
        },
        ("windows", "aarch64") => Platform {
            yt_dlp_asset: "yt-dlp_arm64.exe",
            deno_target: "aarch64-pc-windows-msvc",
            ffmpeg: FfmpegSource::YtDlpBuilds {
                asset: "ffmpeg-master-latest-winarm64-gpl.zip",
            },
        },
        ("linux", "x86_64") => Platform {
            yt_dlp_asset: "yt-dlp_linux",
            deno_target: "x86_64-unknown-linux-gnu",
            ffmpeg: FfmpegSource::YtDlpBuilds {
                asset: "ffmpeg-master-latest-linux64-gpl.tar.xz",
            },
        },
        ("linux", "aarch64") => Platform {
            yt_dlp_asset: "yt-dlp_linux_aarch64",
            deno_target: "aarch64-unknown-linux-gnu",
            ffmpeg: FfmpegSource::YtDlpBuilds {
                asset: "ffmpeg-master-latest-linuxarm64-gpl.tar.xz",
            },
        },
        _ => return None,
    };
    Some(p)
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

pub struct Tools {
    root: PathBuf,
    client: reqwest::Client,
    /// Doesn't follow redirects: "latest" links are resolved by reading `Location`.
    probe: reqwest::Client,
    /// One install/update at a time (Settings button and a video download may race).
    install_lock: tokio::sync::Mutex<()>,
}

impl Tools {
    pub fn new(root: &Path) -> AppResult<Self> {
        fs::create_dir_all(root.join(TMP))?;
        let builder = || {
            reqwest::Client::builder()
                .user_agent(concat!("Recipio/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(20))
                .read_timeout(Duration::from_secs(60))
        };
        Ok(Self {
            root: root.to_path_buf(),
            client: builder().build()?,
            probe: builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            install_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// The HTTP client, shared with other downloads (thumbnails).
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn dir(&self) -> &Path {
        &self.root
    }

    /// Path of an installed executable, e.g. `path("yt-dlp")`.
    pub fn path(&self, name: &str) -> PathBuf {
        self.root.join(exe_name(name))
    }

    fn manifest(&self) -> BTreeMap<Package, Installed> {
        fs::read_to_string(self.root.join(MANIFEST))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save_manifest(&self, manifest: &BTreeMap<Package, Installed>) -> AppResult<()> {
        let json = serde_json::to_string_pretty(manifest).expect("serializable");
        let tmp = self.root.join(format!("{MANIFEST}.new"));
        fs::write(&tmp, json)?;
        fs::rename(tmp, self.root.join(MANIFEST))?;
        Ok(())
    }

    fn is_installed(&self, package: Package, manifest: &BTreeMap<Package, Installed>) -> bool {
        manifest.contains_key(&package)
            && package.executables().iter().all(|e| self.path(e).is_file())
    }

    pub fn all_installed(&self) -> bool {
        let manifest = self.manifest();
        Package::ALL
            .iter()
            .all(|&p| self.is_installed(p, &manifest))
    }

    pub fn status(&self) -> Vec<ToolInfo> {
        let manifest = self.manifest();
        Package::ALL
            .iter()
            .map(|&p| ToolInfo {
                package: p,
                name: p.label().into(),
                version: self
                    .is_installed(p, &manifest)
                    .then(|| manifest[&p].version.clone()),
            })
            .collect()
    }

    /// Installs missing packages, and with `update` also replaces installed ones that
    /// have a newer release. Returns the packages that were (re)installed.
    pub async fn install(
        &self,
        update: bool,
        on_progress: &(dyn Fn(ToolProgress) + Sync),
    ) -> AppResult<Vec<Package>> {
        let _guard = self.install_lock.lock().await;
        let platform = platform(std::env::consts::OS, std::env::consts::ARCH)
            .ok_or(AppError::UnsupportedPlatform)?;
        let mut manifest = self.manifest();
        let mut changed = Vec::new();
        for package in Package::ALL {
            let installed = self.is_installed(package, &manifest);
            if installed && !update {
                continue;
            }
            let release = self.latest(package, &platform).await?;
            if installed && manifest[&package].source_id == release.source_id {
                continue;
            }
            log::info!("installing {} ({})", package.label(), release.source_id);
            for artifact in &release.artifacts {
                self.install_artifact(package, artifact, on_progress)
                    .await?;
            }
            let version = match release.version {
                Some(v) => v,
                None => self.ask_version(package).await,
            };
            manifest.insert(
                package,
                Installed {
                    source_id: release.source_id,
                    version,
                },
            );
            self.save_manifest(&manifest)?;
            changed.push(package);
        }
        Ok(changed)
    }

    /// Finds the newest release of a package for this platform (a few small requests).
    async fn latest(&self, package: Package, platform: &Platform) -> AppResult<Release> {
        match package {
            Package::YtDlp => {
                let tag = self
                    .latest_github_tag("https://github.com/yt-dlp/yt-dlp/releases/latest")
                    .await?;
                let base = format!("https://github.com/yt-dlp/yt-dlp/releases/download/{tag}");
                Ok(Release {
                    source_id: tag.clone(),
                    version: Some(tag),
                    artifacts: vec![Artifact {
                        url: format!("{base}/{}", platform.yt_dlp_asset),
                        checksum: Checksum::Manifest {
                            url: format!("{base}/SHA2-256SUMS"),
                            name: platform.yt_dlp_asset.into(),
                        },
                        archive: Archive::Raw,
                        executables: vec!["yt-dlp"],
                    }],
                })
            }
            Package::Deno => {
                let tag = self
                    .latest_github_tag("https://github.com/denoland/deno/releases/latest")
                    .await?;
                let url = format!(
                    "https://github.com/denoland/deno/releases/download/{tag}/deno-{}.zip",
                    platform.deno_target
                );
                Ok(Release {
                    source_id: tag.clone(),
                    version: Some(tag.trim_start_matches('v').to_string()),
                    artifacts: vec![Artifact {
                        checksum: Checksum::Single {
                            url: format!("{url}.sha256sum"),
                        },
                        url,
                        archive: Archive::Zip,
                        executables: vec!["deno"],
                    }],
                })
            }
            Package::Ffmpeg => match &platform.ffmpeg {
                FfmpegSource::MartinRiedl { arch } => {
                    // The "latest release" link redirects to a build folder such as
                    // /download/macos/arm64/1789931890_9.0.2/ffmpeg.zip.
                    let probe = format!(
                        "https://ffmpeg.martin-riedl.de/redirect/latest/macos/{arch}/release/ffmpeg.zip"
                    );
                    let url = self.redirect_target(&probe).await?;
                    let (base, build) = martin_riedl_build(&url).ok_or_else(|| {
                        AppError::Download(format!("unexpected ffmpeg download location {url}"))
                    })?;
                    let artifact = |name: &'static str| Artifact {
                        url: format!("{base}/{name}.zip"),
                        checksum: Checksum::Single {
                            url: format!("{base}/{name}.zip.sha256"),
                        },
                        archive: Archive::Zip,
                        executables: vec![name],
                    };
                    Ok(Release {
                        version: build.split_once('_').map(|(_, v)| v.to_string()),
                        source_id: build,
                        artifacts: vec![artifact("ffmpeg"), artifact("ffprobe")],
                    })
                }
                FfmpegSource::YtDlpBuilds { asset } => {
                    let base = "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest";
                    let sums_url = format!("{base}/checksums.sha256");
                    let sums = self.fetch_text(&sums_url).await?;
                    // The release tag never changes, so the checksum identifies the build.
                    let sha = manifest_checksum(&sums, asset).ok_or_else(|| {
                        AppError::Download(format!("{asset} missing from {sums_url}"))
                    })?;
                    Ok(Release {
                        source_id: sha,
                        version: None,
                        artifacts: vec![Artifact {
                            url: format!("{base}/{asset}"),
                            checksum: Checksum::Manifest {
                                url: sums_url,
                                name: (*asset).into(),
                            },
                            archive: if asset.ends_with(".zip") {
                                Archive::Zip
                            } else {
                                Archive::TarXz
                            },
                            executables: vec!["ffmpeg", "ffprobe"],
                        }],
                    })
                }
            },
        }
    }

    /// `…/releases/latest` redirects to `…/releases/tag/<tag>`; no API token needed.
    async fn latest_github_tag(&self, url: &str) -> AppResult<String> {
        let target = self.redirect_target(url).await?;
        target
            .rsplit_once("/releases/tag/")
            .map(|(_, tag)| tag.to_string())
            .filter(|tag| !tag.is_empty() && !tag.contains(['/', '?', '#']))
            .ok_or_else(|| AppError::Download(format!("no release tag in {target}")))
    }

    /// Where `url` redirects to, as an absolute URL (without downloading the target).
    async fn redirect_target(&self, url: &str) -> AppResult<String> {
        let resp = self.probe.get(url).send().await?;
        let location = resp
            .status()
            .is_redirection()
            .then(|| resp.headers().get(reqwest::header::LOCATION))
            .flatten()
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                AppError::Download(format!("{url} did not redirect ({})", resp.status()))
            })?;
        let target = resp
            .url()
            .join(location)
            .map_err(|e| AppError::Download(format!("bad redirect from {url}: {e}")))?;
        Ok(target.to_string())
    }

    async fn fetch_text(&self, url: &str) -> AppResult<String> {
        let resp = self.client.get(url).send().await?.error_for_status()?;
        if resp.content_length().is_some_and(|n| n > 1024 * 1024) {
            return Err(AppError::Download(format!("{url} is unexpectedly large")));
        }
        Ok(resp.text().await?)
    }

    async fn install_artifact(
        &self,
        package: Package,
        artifact: &Artifact,
        on_progress: &(dyn Fn(ToolProgress) + Sync),
    ) -> AppResult<()> {
        let expected = match &artifact.checksum {
            Checksum::Manifest { url, name } => {
                manifest_checksum(&self.fetch_text(url).await?, name)
            }
            Checksum::Single { url } => single_checksum(&self.fetch_text(url).await?),
        }
        .ok_or_else(|| AppError::Download(format!("no checksum found for {}", artifact.url)))?;

        let tmp = self
            .root
            .join(TMP)
            .join(format!("{}.download", uuid::Uuid::new_v4()));
        let result = async {
            let actual = self
                .download(package, &artifact.url, &tmp, on_progress)
                .await?;
            if !actual.eq_ignore_ascii_case(&expected) {
                return Err(AppError::Checksum(format!(
                    "{}: expected {expected}, got {actual}",
                    artifact.url
                )));
            }
            let (root, tmp, artifact) = (self.root.clone(), tmp.clone(), artifact.clone());
            tokio::task::spawn_blocking(move || unpack(&root, &tmp, &artifact))
                .await
                .map_err(|e| AppError::Download(format!("unpack task failed: {e}")))?
        }
        .await;
        let _ = fs::remove_file(&tmp);
        result
    }

    /// Streams `url` to `dest` and returns the file's SHA-256 (lowercase hex).
    async fn download(
        &self,
        package: Package,
        url: &str,
        dest: &Path,
        on_progress: &(dyn Fn(ToolProgress) + Sync),
    ) -> AppResult<String> {
        let resp = self.client.get(url).send().await?.error_for_status()?;
        let total = resp.content_length();
        if total.is_some_and(|n| n > MAX_DOWNLOAD_BYTES) {
            return Err(AppError::Download(format!("{url} is unexpectedly large")));
        }
        let mut file = fs::File::create(dest)?;
        let mut hasher = Sha256::new();
        let mut downloaded = 0u64;
        let mut stream = resp.bytes_stream();
        let mut last_report = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            downloaded += chunk.len() as u64;
            if downloaded > MAX_DOWNLOAD_BYTES {
                return Err(AppError::Download(format!("{url} is unexpectedly large")));
            }
            hasher.update(&chunk);
            file.write_all(&chunk)?;
            if downloaded - last_report >= 512 * 1024 || Some(downloaded) == total {
                last_report = downloaded;
                on_progress(ToolProgress {
                    package,
                    name: package.label().into(),
                    downloaded,
                    total,
                });
            }
        }
        file.sync_all()?;
        Ok(hex(&hasher.finalize()))
    }

    /// Runs `<tool> -version` and returns the version word, e.g. "9.0.2" or "N-127654-g7045cc9b80".
    async fn ask_version(&self, package: Package) -> String {
        let exe = self.path(package.executables()[0]);
        let output = crate::process::command(&exe).arg("-version").output().await;
        output
            .ok()
            .and_then(|o| ffmpeg_version(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_else(|| "unknown".into())
    }
}

/// Takes the wanted executables out of a verified download and moves them into place.
fn unpack(root: &Path, download: &Path, artifact: &Artifact) -> AppResult<()> {
    let staged: Vec<(PathBuf, PathBuf)> = artifact
        .executables
        .iter()
        .map(|name| {
            let file = exe_name(name);
            (root.join(format!(".{file}.new")), root.join(file))
        })
        .collect();
    match artifact.archive {
        Archive::Raw => {
            fs::copy(download, &staged[0].0)?;
        }
        Archive::Zip => {
            let mut zip = zip::ZipArchive::new(fs::File::open(download)?)
                .map_err(|e| AppError::Download(format!("bad zip {}: {e}", artifact.url)))?;
            for ((new, _), name) in staged.iter().zip(&artifact.executables) {
                let wanted = exe_name(name);
                let index = (0..zip.len())
                    .find(|&i| {
                        zip.by_index(i).is_ok_and(|f| {
                            f.is_file() && f.name().is_ok_and(|n| file_name_of(&n) == wanted)
                        })
                    })
                    .ok_or_else(|| {
                        AppError::Download(format!("{wanted} not found in {}", artifact.url))
                    })?;
                let mut entry = zip
                    .by_index(index)
                    .map_err(|e| AppError::Download(format!("bad zip entry: {e}")))?;
                copy_limited(&mut entry, new)?;
            }
        }
        Archive::TarXz => {
            let mut found = vec![false; staged.len()];
            let mut tar = tar::Archive::new(xz2::read::XzDecoder::new(fs::File::open(download)?));
            for entry in tar.entries()? {
                let mut entry = entry?;
                if !entry.header().entry_type().is_file() {
                    continue;
                }
                let path = entry.path()?.to_string_lossy().into_owned();
                let name = file_name_of(&path).to_string();
                // Only the copies in bin/ (archives also ship docs and other folders).
                if let Some(i) = artifact
                    .executables
                    .iter()
                    .position(|e| exe_name(e) == name && path.contains("/bin/"))
                {
                    copy_limited(&mut entry, &staged[i].0)?;
                    found[i] = true;
                }
            }
            if let Some(i) = found.iter().position(|f| !f) {
                return Err(AppError::Download(format!(
                    "{} not found in {}",
                    artifact.executables[i], artifact.url
                )));
            }
        }
    }
    for (new, dest) in &staged {
        make_executable(new)?;
        fs::rename(new, dest)?;
    }
    Ok(())
}

fn copy_limited(reader: &mut impl Read, dest: &Path) -> AppResult<()> {
    let mut out = fs::File::create(dest)?;
    let n = std::io::copy(&mut reader.take(MAX_DOWNLOAD_BYTES), &mut out)?;
    if n >= MAX_DOWNLOAD_BYTES {
        return Err(AppError::Download(format!(
            "{} is too large",
            dest.display()
        )));
    }
    Ok(())
}

fn file_name_of(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> AppResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    #[cfg(target_os = "macos")]
    {
        // Our downloads don't get the quarantine flag, but remove it defensively in case
        // something else set it; Gatekeeper would otherwise block the tool. Fails
        // harmlessly when the attribute isn't there.
        let _ = std::process::Command::new("/usr/bin/xattr")
            .args(["-d", "com.apple.quarantine"])
            .arg(path)
            .output();
    }
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> AppResult<()> {
    Ok(())
}

/// Finds `name` in a `<sha256>  <name>` listing (`*name` binary markers allowed).
fn manifest_checksum(listing: &str, name: &str) -> Option<String> {
    listing.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim().trim_start_matches('*');
        (file == name && is_sha256(hash)).then(|| hash.to_ascii_lowercase())
    })
}

/// A checksum file for one asset: the first word, if it is a SHA-256.
fn single_checksum(text: &str) -> Option<String> {
    let hash = text.split_whitespace().next()?;
    is_sha256(hash).then(|| hash.to_ascii_lowercase())
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Splits `https://ffmpeg.martin-riedl.de/download/macos/arm64/1789931890_9.0.2/ffmpeg.zip`
/// into the folder URL and the build name (`1789931890_9.0.2`).
fn martin_riedl_build(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://ffmpeg.martin-riedl.de/download/macos/")?;
    let (base_rel, file) = rest.rsplit_once('/')?;
    let build = base_rel.rsplit('/').next()?;
    let valid = file == "ffmpeg.zip"
        && !build.is_empty()
        && build
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    valid.then(|| {
        (
            format!("https://ffmpeg.martin-riedl.de/download/macos/{base_rel}"),
            build.to_string(),
        )
    })
}

/// "ffmpeg version 9.0.2 Copyright …" → "9.0.2".
fn ffmpeg_version(output: &str) -> Option<String> {
    let rest = output.lines().next()?.split("version ").nth(1)?;
    rest.split_whitespace().next().map(str::to_string)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "7a6fe71d7d680f1fb0ac18d4949873d77db2859e90017c0f44bd94b23be4ee40";
    const B: &str = "33121d192d706e62f684a06f80918e3f3a8ba52ac3a65213b6951c39c7d9f555";

    #[test]
    fn reads_checksum_listings() {
        let listing = format!("{B}  yt-dlp.exe\n{A}  yt-dlp_macos\n{B} *yt-dlp_linux\n");
        assert_eq!(
            manifest_checksum(&listing, "yt-dlp_macos").as_deref(),
            Some(A)
        );
        assert_eq!(
            manifest_checksum(&listing, "yt-dlp_linux").as_deref(),
            Some(B)
        );
        assert_eq!(manifest_checksum(&listing, "yt-dlp"), None);
        assert_eq!(manifest_checksum("nothex  yt-dlp", "yt-dlp"), None);
        assert_eq!(
            single_checksum(&format!("{}  ffmpeg.zip\n", A.to_uppercase())).as_deref(),
            Some(A)
        );
        assert_eq!(single_checksum("<html>Not found</html>"), None);
    }

    #[test]
    fn knows_every_supported_platform() {
        for (os, arch) in [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("windows", "x86_64"),
            ("windows", "aarch64"),
            ("linux", "x86_64"),
            ("linux", "aarch64"),
        ] {
            assert!(platform(os, arch).is_some(), "{os} {arch}");
        }
        assert_eq!(platform("freebsd", "x86_64"), None);
        assert_eq!(platform("linux", "arm"), None);
    }

    #[test]
    fn parses_martin_riedl_redirects() {
        assert_eq!(
            martin_riedl_build(
                "https://ffmpeg.martin-riedl.de/download/macos/arm64/1789931890_9.0.2/ffmpeg.zip"
            ),
            Some((
                "https://ffmpeg.martin-riedl.de/download/macos/arm64/1789931890_9.0.2".into(),
                "1789931890_9.0.2".into()
            ))
        );
        assert_eq!(
            martin_riedl_build("https://evil.example/download/macos/arm64/1_2/ffmpeg.zip"),
            None
        );
        assert_eq!(
            martin_riedl_build(
                "https://ffmpeg.martin-riedl.de/download/macos/arm64/x?y/ffmpeg.zip"
            ),
            None
        );
    }

    #[test]
    fn reads_ffmpeg_version() {
        assert_eq!(
            ffmpeg_version("ffmpeg version N-127654-g7045cc9b80 Copyright (c) 2000-2026\nbuilt")
                .as_deref(),
            Some("N-127654-g7045cc9b80")
        );
        assert_eq!(ffmpeg_version(""), None);
    }

    fn zip_with(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut zip = zip::ZipWriter::new(file.reopen().unwrap());
        for (name, data) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
        file
    }

    fn artifact(archive: Archive, executables: Vec<&'static str>) -> Artifact {
        Artifact {
            url: "https://example.test/a".into(),
            checksum: Checksum::Single { url: String::new() },
            archive,
            executables,
        }
    }

    #[test]
    fn unpacks_executables_from_zip() {
        let dir = tempfile::tempdir().unwrap();
        let zip = zip_with(&[
            ("README.txt", b"docs"),
            (&format!("ffmpeg-x/bin/{}", exe_name("ffmpeg")), b"FF"),
            (&format!("ffmpeg-x/bin/{}", exe_name("ffprobe")), b"FP"),
        ]);
        unpack(
            dir.path(),
            zip.path(),
            &artifact(Archive::Zip, vec!["ffmpeg", "ffprobe"]),
        )
        .unwrap();
        assert_eq!(
            fs::read(dir.path().join(exe_name("ffmpeg"))).unwrap(),
            b"FF"
        );
        assert_eq!(
            fs::read(dir.path().join(exe_name("ffprobe"))).unwrap(),
            b"FP"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join("ffmpeg"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }

        let missing = zip_with(&[("other", b"x")]);
        assert!(
            unpack(
                dir.path(),
                missing.path(),
                &artifact(Archive::Zip, vec!["deno"])
            )
            .is_err()
        );
    }

    #[test]
    fn unpacks_executables_from_tar_xz() {
        let dir = tempfile::tempdir().unwrap();
        let archive = tempfile::NamedTempFile::new().unwrap();
        {
            let xz = xz2::write::XzEncoder::new(archive.reopen().unwrap(), 6);
            let mut tar = tar::Builder::new(xz);
            for (path, data) in [
                ("ffmpeg-x/doc/ffmpeg", &b"manual"[..]),
                ("ffmpeg-x/bin/ffmpeg", b"FF"),
                ("ffmpeg-x/bin/ffprobe", b"FP"),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append_data(&mut header, path, data).unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
        }
        // exe_name adds .exe on Windows, where tar archives aren't used.
        if cfg!(unix) {
            unpack(
                dir.path(),
                archive.path(),
                &artifact(Archive::TarXz, vec!["ffmpeg", "ffprobe"]),
            )
            .unwrap();
            assert_eq!(fs::read(dir.path().join("ffmpeg")).unwrap(), b"FF");
            assert_eq!(fs::read(dir.path().join("ffprobe")).unwrap(), b"FP");
        }
    }

    #[test]
    fn manifest_round_trips_and_detects_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let tools = Tools::new(dir.path()).unwrap();
        assert!(!tools.all_installed());
        let mut m = BTreeMap::new();
        for p in Package::ALL {
            m.insert(
                p,
                Installed {
                    source_id: "s".into(),
                    version: "1.0".into(),
                },
            );
            for e in p.executables() {
                fs::write(tools.path(e), "x").unwrap();
            }
        }
        tools.save_manifest(&m).unwrap();
        assert!(tools.all_installed());
        assert_eq!(tools.status()[0].version.as_deref(), Some("1.0"));
        fs::remove_file(tools.path("ffprobe")).unwrap();
        assert!(!tools.all_installed());
        assert_eq!(tools.status()[1].version, None);
    }

    /// Real download from the publishers; run with `cargo test -- --ignored live_install`.
    /// Set RECIPIO_TOOLS_DIR to keep the result.
    #[test]
    #[ignore = "downloads ~150 MB"]
    fn live_install() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = std::env::var("RECIPIO_TOOLS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| tmp.path().to_path_buf());
        let tools = Tools::new(&dir).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let changed = rt
            .block_on(tools.install(false, &|p: ToolProgress| {
                if p.total == Some(p.downloaded) {
                    eprintln!("{} {} bytes", p.name, p.downloaded);
                }
            }))
            .unwrap();
        eprintln!("installed {changed:?}: {:?}", tools.status());
        assert!(tools.all_installed());
        // Nothing newer appeared in the meantime, so an update changes nothing.
        let again = rt.block_on(tools.install(true, &|_| {})).unwrap();
        assert!(again.is_empty(), "{again:?}");
    }
}
