//! Sending recipes to friends through apps people already use (PLAN.md Phase 8, Q25).
//!
//! On macOS, Messages or Mail opens with the friend as recipient and the file attached
//! (`NSSharingService`), or the system share menu opens (`NSSharingServicePicker`).
//! Elsewhere, a new email to the friend opens in the default mail app (`mailto:`) and the
//! file is shown in its folder to drag in. Files are written to `<app_data>/outbox/`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use ts_rs::TS;

use crate::models::{Friend, ShareMethod};

/// Outbox files are kept this long, so a mail draft can still find its attachment.
const OUTBOX_DAYS: u64 = 7;
/// Where people without Recipio can get it (mentioned in the message text).
const APP_LINK: &str = "https://github.com/bvdon/recipio";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ShareFormat {
    /// A `.recipio` file the friend adds to their Recipio library.
    Recipio,
    /// A PDF anyone can read.
    Pdf,
}

impl ShareFormat {
    pub fn ext(self) -> &'static str {
        match self {
            ShareFormat::Recipio => "recipio",
            ShareFormat::Pdf => "pdf",
        }
    }
}

/// Where the shared file goes.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum ShareTarget {
    Friend {
        #[ts(type = "number")]
        id: i64,
    },
    /// The system share menu (macOS), anchored at this rectangle of the window (CSS
    /// pixels, from the top left); elsewhere the file is shown in its folder.
    Other { anchor: Anchor },
    /// Just save it here (chosen in a save dialog).
    File { path: String },
}

#[derive(Debug, Clone, Copy, Default, Deserialize, TS)]
#[ts(export)]
// Only the macOS share menu uses the position.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct Anchor {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// What the app did with the file, so the UI can say what to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
// Messages, Mail and the share menu are macOS-only.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum Handoff {
    /// Messages opened with the friend and the file.
    Messages,
    /// Mail opened with the friend and the file.
    Mail,
    /// The system share menu opened.
    ShareMenu,
    /// A new email to the friend opened in the default mail app, and the file's folder
    /// opened to drag it in.
    EmailDraft,
    /// The file's folder opened (send it however you like).
    Folder,
    /// Saved where the user chose.
    Saved,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ShareOutcome {
    pub file: String,
    #[ts(type = "number")]
    pub bytes: u64,
    pub handoff: Handoff,
}

pub fn outbox(app_data: &Path) -> PathBuf {
    app_data.join("outbox")
}

/// Startup housekeeping: deletes outbox files older than a week.
pub fn prune_outbox(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let cutoff = SystemTime::now() - Duration::from_secs(OUTBOX_DAYS * 24 * 3600);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t < cutoff);
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// A file name for what's being shared: the recipe's title, or "3 recipes from Rob".
pub fn file_name(titles: &[String], from: &str, format: ShareFormat) -> String {
    let base = match (titles, from.trim()) {
        ([one], _) => one.clone(),
        (many, "") => format!("{} recipes", many.len()),
        (many, from) => format!("{} recipes from {from}", many.len()),
    };
    let clean: String = base
        .chars()
        .filter(|c| !c.is_control() && !r#"\/:*?"<>|"#.contains(*c))
        .collect();
    let clean = clean.trim().trim_matches('.').trim();
    let clean: String = clean.chars().take(80).collect();
    let clean = if clean.is_empty() {
        "Recipes".into()
    } else {
        clean
    };
    format!("{clean}.{}", format.ext())
}

/// `dir/name`, or `dir/name 2.ext` etc. if that's taken.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    (2..)
        .map(|n| dir.join(format!("{stem} {n}.{ext}")))
        .find(|p| !p.exists())
        .expect("a free name")
}

pub fn subject(titles: &[String], from: &str) -> String {
    match (titles, from.trim()) {
        ([one], "") => format!("Recipe: {one}"),
        ([one], from) => format!("Recipe from {from}: {one}"),
        (_, "") => "Recipes".into(),
        (_, from) => format!("Recipes from {from}"),
    }
}

/// The message that goes with the file: the user's note, plus how to open it.
pub fn message(note: &str, format: ShareFormat, count: usize) -> String {
    let hint = match format {
        ShareFormat::Recipio => format!(
            "Open the attached .recipio file with Recipio to add {} to your recipes ({APP_LINK}).",
            if count == 1 { "it" } else { "them" }
        ),
        ShareFormat::Pdf => String::new(),
    };
    [note.trim(), hint.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Characters left as they are in the address part of a `mailto:` link.
const ADDRESS: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'@')
    .remove(b'.')
    .remove(b'-')
    .remove(b'_')
    .remove(b'+');

pub fn mailto(address: &str, subject: &str, body: &str) -> String {
    let enc = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
    format!(
        "mailto:{}?subject={}&body={}",
        utf8_percent_encode(address.trim(), ADDRESS),
        enc(subject),
        enc(body)
    )
}

/// Hands a written file to a friend's app (see the module docs). Never fails: when the
/// preferred way isn't available, the file's folder is shown instead.
pub fn send_to_friend(
    app: &AppHandle,
    file: &Path,
    friend: &Friend,
    subject: &str,
    text: &str,
) -> Handoff {
    #[cfg(target_os = "macos")]
    {
        let service = match friend.method {
            ShareMethod::Messages => mac::Service::Messages,
            ShareMethod::Email => mac::Service::Mail,
        };
        match mac::compose(app, service, &friend.address, subject, text, file) {
            Ok(()) => {
                return match friend.method {
                    ShareMethod::Messages => Handoff::Messages,
                    ShareMethod::Email => Handoff::Mail,
                };
            }
            Err(e) => log::warn!("could not open {service:?}: {e}"),
        }
    }
    email_draft(app, file, friend, subject, text)
}

/// A `mailto:` draft to the friend (if they have an email address) and the file's folder.
fn email_draft(
    app: &AppHandle,
    file: &Path,
    friend: &Friend,
    subject: &str,
    text: &str,
) -> Handoff {
    reveal(app, file);
    if friend.method == ShareMethod::Email || friend.address.contains('@') {
        let body = format!(
            "{text}\n\n(Attach the file {} from the folder that just opened.)",
            file_label(file)
        );
        match app
            .opener()
            .open_url(mailto(&friend.address, subject, &body), None::<&str>)
        {
            Ok(()) => return Handoff::EmailDraft,
            Err(e) => log::warn!("could not open a mail draft: {e}"),
        }
    }
    Handoff::Folder
}

fn file_label(file: &Path) -> String {
    file.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The system share menu on macOS; elsewhere the file's folder.
pub fn send_other(app: &AppHandle, file: &Path, text: &str, anchor: Anchor) -> Handoff {
    #[cfg(target_os = "macos")]
    match mac::picker(app, file, text, anchor) {
        Ok(()) => return Handoff::ShareMenu,
        Err(e) => log::warn!("could not open the share menu: {e}"),
    }
    let _ = (text, anchor);
    reveal(app, file);
    Handoff::Folder
}

pub fn reveal(app: &AppHandle, file: &Path) {
    if let Err(e) = app.opener().reveal_item_in_dir(file) {
        log::warn!("could not show {}: {e}", file.display());
    }
}

/// Runs `f` on the main thread (AppKit requires it) and waits for its result.
#[cfg(target_os = "macos")]
fn on_main<T: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce(&AppHandle) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.send(f(&handle));
    })
    .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(10))
        .map_err(|_| "the main thread didn't answer".to_string())?
}

#[cfg(target_os = "macos")]
mod mac {
    use std::path::Path;

    use objc2::AnyThread;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{
        NSSharingService, NSSharingServiceNameComposeEmail, NSSharingServiceNameComposeMessage,
        NSSharingServicePicker, NSView,
    };
    use objc2_foundation::{NSArray, NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSURL};
    use tauri::{AppHandle, Manager};

    use super::{Anchor, on_main};

    #[derive(Debug, Clone, Copy)]
    pub enum Service {
        Messages,
        Mail,
    }

    /// The message text and the file, as sharing items.
    fn items(text: &str, file: &Path) -> Result<Retained<NSArray<AnyObject>>, String> {
        let path = file.to_str().ok_or("path isn't UTF-8")?;
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        let mut list: Vec<Retained<AnyObject>> = Vec::new();
        if !text.is_empty() {
            list.push(Retained::into_super(Retained::into_super(
                NSString::from_str(text),
            )));
        }
        list.push(Retained::into_super(Retained::into_super(url)));
        Ok(NSArray::from_retained_slice(&list))
    }

    pub fn compose(
        app: &AppHandle,
        service: Service,
        recipient: &str,
        subject: &str,
        text: &str,
        file: &Path,
    ) -> Result<(), String> {
        let (recipient, subject, text) =
            (recipient.to_string(), subject.to_string(), text.to_string());
        let file = file.to_path_buf();
        on_main(app, move |_| {
            let name = match service {
                Service::Messages => unsafe { NSSharingServiceNameComposeMessage },
                Service::Mail => unsafe { NSSharingServiceNameComposeEmail },
            };
            let sharing =
                NSSharingService::sharingServiceNamed(name).ok_or("service not available")?;
            let to = NSArray::from_retained_slice(&[NSString::from_str(&recipient)]);
            sharing.setRecipients(Some(&to));
            if matches!(service, Service::Mail) {
                sharing.setSubject(Some(&NSString::from_str(&subject)));
            }
            let items = items(&text, &file)?;
            // SAFETY: items are an NSString and a file NSURL, both NSPasteboardWriting.
            unsafe {
                if !sharing.canPerformWithItems(Some(&items)) {
                    return Err("the service can't send these items (not set up?)".into());
                }
                sharing.performWithItems(&items);
            }
            Ok(())
        })
    }

    pub fn picker(app: &AppHandle, file: &Path, text: &str, anchor: Anchor) -> Result<(), String> {
        let (text, file) = (text.to_string(), file.to_path_buf());
        on_main(app, move |app| {
            let window = app.get_webview_window("main").ok_or("no main window")?;
            let ptr = window.ns_view().map_err(|e| e.to_string())?;
            // SAFETY: Tauri returns the window's live content NSView; used on the main
            // thread while the window exists.
            let view: &NSView = unsafe { &*(ptr as *const NSView) };
            let height = view.frame().size.height;
            // CSS pixels from the top left; AppKit views count from the bottom unless flipped.
            let y = if view.isFlipped() {
                anchor.y
            } else {
                height - anchor.y - anchor.height
            };
            let rect = NSRect::new(
                NSPoint::new(anchor.x, y),
                NSSize::new(anchor.width.max(1.0), anchor.height.max(1.0)),
            );
            let items = items(&text, &file)?;
            // SAFETY: valid sharing items (see `compose`).
            let picker = unsafe {
                NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items)
            };
            picker.showRelativeToRect_ofView_preferredEdge(rect, view, NSRectEdge::MinY);
            Ok(())
        })
    }
}

/// `.recipio` files the app was asked to open (launch arguments or macOS "open" events).
pub fn recipio_files<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .filter_map(|a| {
            let a = a.as_ref();
            let path = match a.strip_prefix("file://") {
                Some(_) => url::Url::parse(a).ok()?.to_file_path().ok()?,
                None => PathBuf::from(a),
            };
            let is_recipio = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("recipio"));
            (is_recipio && path.is_file()).then(|| path.to_string_lossy().into_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_files_and_messages() {
        let one = vec!["Mom's Pie: the best!".to_string()];
        let three = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(
            file_name(&one, "Rob", ShareFormat::Recipio),
            "Mom's Pie the best!.recipio"
        );
        assert_eq!(
            file_name(&three, "Rob", ShareFormat::Pdf),
            "3 recipes from Rob.pdf"
        );
        assert_eq!(
            file_name(&three, " ", ShareFormat::Recipio),
            "3 recipes.recipio"
        );
        assert_eq!(
            file_name(&["...".into()], "", ShareFormat::Pdf),
            "Recipes.pdf"
        );
        assert_eq!(
            subject(&one, "Rob"),
            "Recipe from Rob: Mom's Pie: the best!"
        );
        assert_eq!(subject(&three, ""), "Recipes");
        let text = message("  Try these!  ", ShareFormat::Recipio, 3);
        assert!(text.starts_with("Try these!\n\nOpen the attached .recipio file"));
        assert!(text.contains("add them"));
        assert_eq!(message("", ShareFormat::Pdf, 1), "");
        assert_eq!(
            mailto("sam@example.com", "Recipes from Rob", "Hi & bye"),
            "mailto:sam@example.com?subject=Recipes%20from%20Rob&body=Hi%20%26%20bye"
        );
    }

    #[test]
    fn picks_free_names_and_prunes_the_outbox() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert_eq!(unique_path(dir, "Pie.recipio"), dir.join("Pie.recipio"));
        fs::write(dir.join("Pie.recipio"), b"x").unwrap();
        fs::write(dir.join("Pie 2.recipio"), b"x").unwrap();
        assert_eq!(unique_path(dir, "Pie.recipio"), dir.join("Pie 3.recipio"));

        let old = fs::File::create(dir.join("old.recipio")).unwrap();
        old.set_modified(SystemTime::now() - Duration::from_secs(8 * 24 * 3600))
            .unwrap();
        drop(old);
        prune_outbox(dir);
        assert!(!dir.join("old.recipio").exists());
        assert!(dir.join("Pie.recipio").exists());
    }

    #[test]
    fn finds_recipio_files_in_arguments() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("From Sam.recipio");
        fs::write(&file, b"x").unwrap();
        let url = url::Url::from_file_path(&file).unwrap().to_string();
        let found = recipio_files([
            "/Applications/Recipio.app/Contents/MacOS/recipio",
            "--flag",
            file.to_str().unwrap(),
            &url,
            "/missing/file.recipio",
            tmp.path().join("notes.txt").to_str().unwrap(),
        ]);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|f| f.ends_with("From Sam.recipio")));
    }
}
