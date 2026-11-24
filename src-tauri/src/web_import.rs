//! Web import: fetches a recipe page (or reads one the user saved from their browser), reads its schema.org data (`jsonld`), matches its
//! categories/keywords to existing labels and stages its photo. Nothing is saved here —
//! the editor shows the result for review.

use std::path::Path;
use std::time::Duration;

use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{StatusCode, Url};

use scraper::{Html, Selector};
use tokio::io::AsyncReadExt;

use crate::AppState;
use crate::db::labels;
use crate::error::{AppError, AppResult};
use crate::models::{ImportedRecipe, LabelKind, WebImport};
use crate::{ingredient_parser, jsonld, video};

const PAGE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_PAGE_BYTES: usize = 5 * 1024 * 1024;
/// Saved pages may be larger than fetched ones: browsers can inline a page's assets.
const MAX_SAVED_PAGE_BYTES: usize = 20 * 1024 * 1024;
/// Stands in as the base address of a saved page that doesn't say where it came from.
/// `.invalid` never resolves (RFC 2606); images that end up on it are skipped.
const UNKNOWN_PAGE: &str = "https://saved-page.invalid/";
const IMAGE_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
/// Image candidates tried before giving up on the photo.
const IMAGE_TRIES: usize = 3;
/// Many recipe sites turn away requests that don't look like a browser (PLAN.md Q16).
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
                          (KHTML, like Gecko) Version/18.5 Safari/605.1.15";

/// Reads the recipe at `text` (a page address as typed or pasted).
pub async fn import(state: &AppState, text: &str) -> AppResult<WebImport> {
    let url = page_url(text)?;
    log::info!("importing recipe from {url}");
    let client = client()?;
    let (html, final_url) = fetch_page(&client, &url).await?;
    from_html(state, &client, &html, &final_url, url.to_string()).await
}

/// Reads the recipe from a page the user saved in their browser, for sites that refuse
/// the app's own request. `typed_url` is whatever is in the dialog's address field: it is
/// only used when the file doesn't name its own address, because the file may be a
/// different page from the one last typed. The page itself is never fetched, only its photo.
pub async fn import_saved(state: &AppState, path: &Path, typed_url: &str) -> AppResult<WebImport> {
    log::info!("importing recipe from a saved page");
    let html = read_saved_page(path).await?;
    let typed = page_url(typed_url).ok();
    let source = saved_page_address(&html, typed.as_ref()).or(typed);
    let source_url = source.as_ref().map(Url::to_string).unwrap_or_default();
    let base = source
        .unwrap_or_else(|| Url::parse(UNKNOWN_PAGE).expect("the placeholder address is valid"));
    from_html(state, &client()?, &html, &base, source_url).await
}

/// Turns a page's HTML into an import result. `base` resolves the page's relative links;
/// `source_url` is the address kept with the recipe (empty if unknown).
async fn from_html(
    state: &AppState,
    client: &reqwest::Client,
    html: &str,
    base: &Url,
    source_url: String,
) -> AppResult<WebImport> {
    let Some(found) = jsonld::extract(html, base) else {
        log::info!("no recipe data on {base}");
        return Ok(WebImport::NoRecipe { source_url });
    };

    let mut recipe = ImportedRecipe {
        source_url,
        title: found.title,
        description: found.description,
        servings: found.servings,
        prep_minutes: found.prep_minutes,
        cook_minutes: found.cook_minutes,
        ingredients: found
            .ingredients
            .iter()
            .filter_map(|l| ingredient_parser::parse_line(l))
            .collect(),
        steps: found.steps,
        nutrition: found.nutrition,
        youtube_url: found.videos.iter().find_map(|u| video::youtube_url(u)),
        ..Default::default()
    };
    {
        let conn = state.db();
        let categories = labels::list(&conn, LabelKind::Category)?;
        let tags = labels::list(&conn, LabelKind::Tag)?;
        for name in &found.labels {
            let existing = |list: &[crate::models::Label]| {
                list.iter()
                    .find(|l| l.name.eq_ignore_ascii_case(name))
                    .map(|l| l.name.clone())
            };
            if let Some(c) = existing(&categories) {
                push_unique(&mut recipe.categories, c);
            } else if let Some(t) = existing(&tags) {
                push_unique(&mut recipe.tags, t);
            }
        }
    }

    let usable_images = found.images.iter().filter(|i| !i.starts_with(UNKNOWN_PAGE));
    for image in usable_images.take(IMAGE_TRIES) {
        match fetch_image(state, client, image).await {
            Ok(staged) => {
                recipe.photo = Some(staged);
                break;
            }
            Err(e) => log::warn!("recipe image {image} not used: {e:?}"),
        }
    }
    if recipe.photo.is_none() && !found.images.is_empty() {
        recipe
            .warnings
            .push("The recipe’s photo couldn’t be downloaded.".into());
    }
    if recipe.ingredients.is_empty() {
        recipe
            .warnings
            .push("The page didn’t list any ingredients.".into());
    }
    if recipe.steps.is_empty() {
        recipe
            .warnings
            .push("The page didn’t list any steps.".into());
    }
    Ok(WebImport::Found {
        recipe: Box::new(recipe),
    })
}

fn push_unique(list: &mut Vec<String>, name: String) {
    if !list.contains(&name) {
        list.push(name);
    }
}

/// Checks a typed or pasted page address; a missing scheme means https.
pub fn page_url(text: &str) -> AppResult<Url> {
    let invalid = || AppError::Validation("That doesn’t look like a web page address.".into());
    let text = text.trim();
    if text.is_empty() || text.len() > 2048 || text.contains(char::is_whitespace) {
        return Err(invalid());
    }
    let url = match Url::parse(text) {
        Ok(url) => url,
        Err(url::ParseError::RelativeUrlWithoutBase) => {
            Url::parse(&format!("https://{text}")).map_err(|_| invalid())?
        }
        Err(_) => return Err(invalid()),
    };
    let ok = matches!(url.scheme(), "http" | "https")
        && url
            .host_str()
            .is_some_and(|h| h.contains('.') || h.starts_with('['))
        && url.username().is_empty()
        && url.password().is_none();
    if !ok {
        return Err(invalid());
    }
    if !is_public(&url) {
        return Err(AppError::Validation(
            "Only pages on the public internet can be imported.".into(),
        ));
    }
    Ok(url)
}

/// False for addresses on this computer or the local network. A page's image URLs and
/// redirects are chosen by the page, so they must not reach into the user's network.
fn is_public(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d != "localhost"
                && ![".localhost", ".local", ".internal", ".home.arpa"]
                    .iter()
                    .any(|s| d.ends_with(s))
        }
        Some(url::Host::Ipv4(ip)) => {
            !(ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                // Carrier-grade NAT, 100.64.0.0/10.
                || (ip.octets()[0] == 100 && (ip.octets()[1] & 0xC0) == 64))
        }
        Some(url::Host::Ipv6(ip)) => {
            let first = ip.segments()[0];
            !(ip.is_loopback()
                || ip.is_unspecified()
                || (first & 0xFE00) == 0xFC00 // unique local
                || (first & 0xFFC0) == 0xFE80 // link local
                || ip.to_ipv4_mapped().is_some())
        }
        None => false,
    }
}

/// A fresh client per import: cookies (for consent redirects) don't outlive it.
fn client() -> AppResult<reqwest::Client> {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
    );
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
    let redirects = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            attempt.error("too many redirects")
        } else if !matches!(attempt.url().scheme(), "http" | "https") || !is_public(attempt.url()) {
            attempt.error("redirect to a non-public address")
        } else {
            attempt.follow()
        }
    });
    Ok(reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .default_headers(headers)
        .cookie_store(true)
        .redirect(redirects)
        .connect_timeout(Duration::from_secs(10))
        .build()?)
}

/// The page's HTML (decoded to text) and its final URL after redirects.
async fn fetch_page(client: &reqwest::Client, url: &Url) -> AppResult<(String, Url)> {
    let resp = client
        .get(url.clone())
        .timeout(PAGE_TIMEOUT)
        .send()
        .await
        .map_err(request_error)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(status_error(status));
    }
    let final_url = resp.url().clone();
    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let is_html = content_type.is_empty()
        || ["text/html", "application/xhtml+xml", "text/plain"]
            .iter()
            .any(|t| content_type.starts_with(t));
    if !is_html {
        return Err(import_error(
            "That link isn’t a web page.",
            format!("content type {content_type}"),
        ));
    }
    let Some(body) = read_capped(resp, MAX_PAGE_BYTES)
        .await
        .map_err(request_error)?
    else {
        return Err(import_error(
            "That page is too large to read.",
            format!("over {MAX_PAGE_BYTES} bytes"),
        ));
    };
    Ok((decode(&body, &content_type), final_url))
}

/// Reads and decodes a saved HTML file, explaining the formats Recipio can't use.
async fn read_saved_page(path: &Path) -> AppResult<String> {
    // Checked before opening, since opening a pipe could wait forever.
    if !tokio::fs::metadata(path).await?.is_file() {
        return Err(AppError::Validation("Please choose a file.".into()));
    }
    // Read one byte past the limit so an oversized file is noticed without loading it all.
    let mut body = Vec::new();
    tokio::fs::File::open(path)
        .await?
        .take(MAX_SAVED_PAGE_BYTES as u64 + 1)
        .read_to_end(&mut body)
        .await?;
    if body.len() > MAX_SAVED_PAGE_BYTES {
        return Err(import_error(
            "That file is too large to read.",
            format!("over {MAX_SAVED_PAGE_BYTES} bytes"),
        ));
    }
    saved_page_text(&body)
}

/// The HTML of a saved page, judged by its content rather than its file extension.
fn saved_page_text(body: &[u8]) -> AppResult<String> {
    // Safari's default format is a binary property list.
    if body.starts_with(b"bplist") {
        return Err(AppError::Validation(
            "That’s a Safari web archive. Save the page again and choose “Page Source” as the \
             format."
                .into(),
        ));
    }
    let html = decode(body, "");
    // ASCII lowercasing keeps byte offsets, and the headers checked here are all ASCII.
    let start: String = html
        .chars()
        .take(4096)
        .collect::<String>()
        .to_ascii_lowercase();
    // Chrome and Edge's "single file" format is a MIME message with encoded HTML inside.
    if start.contains("multipart/related") && start.contains("mime-version:") {
        return Err(AppError::Validation(
            "That’s a single-file web page. Save the page again and choose “Webpage, HTML Only” \
             as the format."
                .into(),
        ));
    }
    // A NUL means binary data; a page always has a tag near the top.
    if start.contains('\0') || !start.contains('<') {
        return Err(AppError::Validation(
            "That file isn’t a saved web page.".into(),
        ));
    }
    // Saved before the browser check finished: this is Cloudflare's interstitial.
    if start.contains("<title>just a moment...</title>") {
        return Err(AppError::Validation(
            "That file is the site’s “Just a moment…” page. Wait until the recipe appears in \
             your browser, then save again."
                .into(),
        ));
    }
    Ok(html)
}

/// The address a saved page gives for itself (`canonical`, else `og:url`). It is held to
/// the same checks as a typed one, since the file's content is untrusted.
fn saved_page_address(html: &str, typed: Option<&Url>) -> Option<Url> {
    let doc = Html::parse_document(html);
    [
        (r#"link[rel~="canonical" i]"#, "href"),
        (r#"meta[property="og:url" i]"#, "content"),
    ]
    .iter()
    .find_map(|(selector, attribute)| {
        let selector = Selector::parse(selector).expect("valid selector");
        let address = doc.select(&selector).next()?.attr(attribute)?.trim();
        // Relative addresses only make sense next to a known one.
        let absolute = match typed {
            Some(base) => base.join(address).ok()?,
            None => Url::parse(address).ok()?,
        };
        page_url(absolute.as_str()).ok()
    })
}

/// The body, or `None` if it's larger than `max` bytes.
async fn read_capped(mut resp: reqwest::Response, max: usize) -> reqwest::Result<Option<Vec<u8>>> {
    if resp.content_length().is_some_and(|n| n > max as u64) {
        return Ok(None);
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if body.len() + chunk.len() > max {
            return Ok(None);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(body))
}

/// Decodes HTML using the charset from the header, else a `<meta charset>`, else UTF-8.
fn decode(body: &[u8], content_type: &str) -> String {
    let head = String::from_utf8_lossy(&body[..body.len().min(4096)]).to_ascii_lowercase();
    let label = charset_of(content_type).or_else(|| charset_of(&head));
    let encoding = label
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(body).0.into_owned()
}

fn charset_of(text: &str) -> Option<String> {
    let (_, rest) = text.split_once("charset=")?;
    let label: String = rest
        .trim_start_matches(['"', '\''])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
        .collect();
    (!label.is_empty()).then_some(label)
}

/// Downloads an image the page named and stages it like a chosen photo.
async fn fetch_image(state: &AppState, client: &reqwest::Client, url: &str) -> AppResult<String> {
    let url = Url::parse(url).map_err(|e| AppError::Download(format!("bad image URL: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") || !is_public(&url) {
        return Err(AppError::Download("image URL not allowed".into()));
    }
    let resp = client
        .get(url)
        .header(
            ACCEPT,
            "image/avif,image/webp,image/png,image/jpeg,image/*;q=0.8",
        )
        .timeout(IMAGE_TIMEOUT)
        .send()
        .await?
        .error_for_status()?;
    let data = read_capped(resp, MAX_IMAGE_BYTES)
        .await?
        .ok_or_else(|| AppError::Download("image too large".into()))?;
    let ext =
        image_type(&data).ok_or_else(|| AppError::Download("not a supported image".into()))?;
    state.media.stage_bytes(ext, &data)
}

/// File extension for the image formats the app shows, judged by the bytes themselves
/// (servers often send the wrong content type).
fn image_type(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("webp")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("gif")
    } else if data.len() > 12 && &data[4..8] == b"ftyp" && matches!(&data[8..12], b"avif" | b"avis")
    {
        Some("avif")
    } else {
        None
    }
}

fn import_error(message: &str, detail: String) -> AppError {
    AppError::Import {
        message: message.into(),
        detail,
    }
}

fn request_error(e: reqwest::Error) -> AppError {
    let message = if e.is_timeout() {
        "The site took too long to respond. Please try again later."
    } else if e.is_redirect() {
        "The page redirected somewhere Recipio won’t follow."
    } else {
        "Couldn’t reach that site. Check the address and your internet connection."
    };
    import_error(message, format!("{e:?}"))
}

fn status_error(status: StatusCode) -> AppError {
    let detail = format!("HTTP {status}");
    let message = match status.as_u16() {
        401 | 403 | 429 | 503 => return AppError::ImportBlocked { detail },
        404 | 410 => "That page wasn’t found. Please check the address.".to_string(),
        code => format!("The site returned an error (HTTP {code}). Please try again later."),
    };
    AppError::Import { message, detail }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Result<String, String> {
        page_url(text)
            .map(|u| u.to_string())
            .map_err(|e| e.user_message())
    }

    #[test]
    fn checks_page_addresses() {
        assert_eq!(
            url(" https://www.example.com/recipe/1 ").unwrap(),
            "https://www.example.com/recipe/1"
        );
        assert_eq!(
            url("example.com/pancakes").unwrap(),
            "https://example.com/pancakes"
        );
        assert_eq!(url("http://example.com/").unwrap(), "http://example.com/");
        for bad in [
            "",
            "pancakes",
            "ftp://example.com/x",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://user:pw@example.com/",
            "https://exa mple.com/",
        ] {
            assert!(url(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn refuses_local_addresses() {
        for local in [
            "http://localhost:8080/",
            "http://127.0.0.1/",
            "http://192.168.1.1/admin",
            "http://10.0.0.5/",
            "http://169.254.169.254/latest",
            "http://100.64.0.1/",
            "http://[::1]/",
            "http://[fe80::1]/",
            "http://[fd00::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://printer.local/",
            "http://router.home.arpa/",
        ] {
            let u = Url::parse(local).unwrap();
            assert!(!is_public(&u), "{local} should be refused");
        }
        for public in [
            "https://example.com/",
            "http://93.184.215.14/",
            "http://[2606:2800::1]/",
        ] {
            assert!(is_public(&Url::parse(public).unwrap()), "{public}");
        }
        assert!(
            url("http://192.168.1.1/")
                .unwrap_err()
                .contains("public internet")
        );
    }

    #[test]
    fn decodes_declared_charsets() {
        let latin1 = b"<html><head><meta charset=\"iso-8859-1\"></head><body>Cr\xe8me br\xfbl\xe9e</body></html>";
        assert!(decode(latin1, "text/html").contains("Crème brûlée"));
        assert!(decode(latin1, "text/html; charset=windows-1252").contains("Crème brûlée"));
        assert!(decode("Crème".as_bytes(), "text/html").contains("Crème"));
        assert_eq!(charset_of("text/html; charset=UTF-8"), Some("UTF-8".into()));
        assert_eq!(charset_of("text/html"), None);
    }

    #[test]
    fn recognises_image_bytes() {
        assert_eq!(image_type(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Some("jpg"));
        assert_eq!(image_type(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Some("png"));
        assert_eq!(image_type(b"RIFF\x24\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(image_type(b"GIF89a\x01\0\x01\0"), Some("gif"));
        assert_eq!(image_type(b"\0\0\0\x1cftypavif\0\0\0\0"), Some("avif"));
        assert_eq!(image_type(b"<!DOCTYPE html><html>"), None);
        assert_eq!(image_type(b""), None);
    }

    #[test]
    fn explains_http_errors() {
        let msg = |code| status_error(StatusCode::from_u16(code).unwrap()).user_message();
        assert!(msg(404).contains("wasn’t found"));
        assert!(msg(403).contains("refused"));
        assert!(matches!(
            status_error(StatusCode::FORBIDDEN),
            AppError::ImportBlocked { .. }
        ));
        assert!(msg(500).contains("HTTP 500"));
    }

    #[tokio::test]
    async fn reads_a_saved_page() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wprm_graph.html");
        let html = read_saved_page(&path).await.unwrap();
        let base = Url::parse(UNKNOWN_PAGE).unwrap();
        assert!(
            !jsonld::extract(&html, &base)
                .unwrap()
                .ingredients
                .is_empty()
        );
        // A folder is not a page.
        assert!(read_saved_page(path.parent().unwrap()).await.is_err());
        // Nor is a file past the size limit.
        let big = tempfile::NamedTempFile::new().unwrap();
        big.as_file()
            .set_len(MAX_SAVED_PAGE_BYTES as u64 + 1)
            .unwrap();
        let error = read_saved_page(big.path()).await.unwrap_err();
        assert!(error.user_message().contains("too large"));
    }

    #[test]
    fn explains_unusable_saved_files() {
        let message = |body: &[u8]| saved_page_text(body).unwrap_err().user_message();
        assert!(message(b"bplist00\xd1\x01").contains("Page Source"));
        assert!(
            message(b"MIME-Version: 1.0\r\nContent-Type: multipart/related;\r\n\r\n<html>")
                .contains("HTML Only")
        );
        assert!(message(b"\x89PNG\r\n\x1a\n\0\0<").contains("isn’t a saved web page"));
        assert!(message(b"flour, sugar").contains("isn’t a saved web page"));
        assert!(message(b"").contains("isn’t a saved web page"));
        assert!(message(b"<html><title>Just a moment...</title>").contains("recipe appears"));
        assert!(saved_page_text("<html>Crème".as_bytes()).is_ok());
    }

    #[test]
    fn finds_a_saved_pages_address() {
        let address = |html: &str, typed: Option<&str>| {
            let typed = typed.map(|t| Url::parse(t).unwrap());
            saved_page_address(html, typed.as_ref()).map(|u| u.to_string())
        };
        let canonical = r#"<link rel="canonical" href="https://example.com/soup/">"#;
        let og = r#"<meta property="og:url" content="https://example.com/stew/">"#;
        let relative = r#"<link rel="canonical" href="/pie/">"#;
        let typed = Some("https://example.com/other");
        // The file's own address wins over the typed one.
        assert_eq!(
            address(&format!("{og}{canonical}"), typed).unwrap(),
            "https://example.com/soup/"
        );
        assert_eq!(address(og, None).unwrap(), "https://example.com/stew/");
        assert_eq!(
            address(relative, typed).unwrap(),
            "https://example.com/pie/"
        );
        assert_eq!(address(relative, None), None);
        assert_eq!(address("<html>", typed), None);
        for bad in ["file:///etc/passwd", "http://192.168.1.1/", "javascript:x"] {
            let html = format!(r#"<link rel="canonical" href="{bad}">"#);
            assert_eq!(address(&html, typed), None, "{bad}");
        }
    }

    /// Fetches real pages: `RECIPIO_IMPORT_URLS="url1 url2" cargo test -- --ignored live_import --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_import() {
        let urls = std::env::var("RECIPIO_IMPORT_URLS").expect("set RECIPIO_IMPORT_URLS");
        let client = client().unwrap();
        for text in urls.split_whitespace() {
            let url = page_url(text).unwrap();
            match fetch_page(&client, &url).await {
                Ok((html, final_url)) => match jsonld::extract(&html, &final_url) {
                    Some(r) => println!(
                        "OK  {url}\n    {:?}: {} ingredients, {} steps, servings {:?}, prep {:?}, cook {:?}, {} images, videos {:?}, labels {:?}\n    first: {:?} / {:?}",
                        r.title,
                        r.ingredients.len(),
                        r.steps.len(),
                        r.servings,
                        r.prep_minutes,
                        r.cook_minutes,
                        r.images.len(),
                        r.videos,
                        r.labels,
                        r.ingredients.first(),
                        r.steps.first()
                    ),
                    None => println!("--  {url}: no recipe data"),
                },
                Err(e) => println!("ERR {url}: {e:?}"),
            }
        }
    }
}
