//! Web import: fetches a recipe page, reads its schema.org data (`jsonld`), matches its
//! categories/keywords to existing labels and stages its photo. Nothing is saved here —
//! the editor shows the result for review.

use std::time::Duration;

use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{StatusCode, Url};

use crate::AppState;
use crate::db::labels;
use crate::error::{AppError, AppResult};
use crate::models::{ImportedRecipe, LabelKind, WebImport};
use crate::{ingredient_parser, jsonld, video};

const PAGE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_PAGE_BYTES: usize = 5 * 1024 * 1024;
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
    let source_url = url.to_string();
    let Some(found) = jsonld::extract(&html, &final_url) else {
        log::info!("no recipe data on {final_url}");
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

    for image in found.images.iter().take(IMAGE_TRIES) {
        match fetch_image(state, &client, image).await {
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
    let message = match status.as_u16() {
        404 | 410 => "That page wasn’t found. Please check the address.".to_string(),
        401 | 403 | 429 | 503 => "The site refused the request. Some sites block apps from \
                                 reading their pages; you can still add the recipe by hand."
            .to_string(),
        code => format!("The site returned an error (HTTP {code}). Please try again later."),
    };
    AppError::Import {
        message,
        detail: format!("HTTP {status}"),
    }
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
        assert!(msg(500).contains("HTTP 500"));
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
