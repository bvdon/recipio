//! A recipe as a PDF (the recipe page's "PDF" button): text, photo and details. The video
//! is never included — not the file, the link or its bookmarks.
//!
//! krilla writes the PDF; the layout (wrapping, page breaks) is done here. Text is measured
//! with rustybuzz, the shaper krilla draws with, so wrapped lines fit exactly.

use krilla::Document;
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect, Size, Transform};
use krilla::image::Image;
use krilla::metadata::Metadata;
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::Fill;
use krilla::text::{Font, TextDirection};

use crate::error::{AppError, AppResult};
use crate::models::Recipe;

static REGULAR: &[u8] = include_bytes!("../assets/fonts/NotoSans-Regular.ttf");
static BOLD: &[u8] = include_bytes!("../assets/fonts/NotoSans-Bold.ttf");

/// US Letter or A4, in points.
const LETTER: (f32, f32) = (612.0, 792.0);
const A4: (f32, f32) = (595.28, 841.89);
const MARGIN: f32 = 54.0;
const FOOTER_GAP: f32 = 24.0;
const MAX_PHOTO_HEIGHT: f32 = 260.0;

const TEXT: (u8, u8, u8) = (36, 33, 29);
const MUTED: (u8, u8, u8) = (119, 111, 101);
const ACCENT: (u8, u8, u8) = (181, 85, 43);

/// Regions using US Letter paper; everyone else gets A4.
fn uses_letter(locale: &str) -> bool {
    let region = locale
        .split(['-', '_'])
        .nth(1)
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(
        region.as_str(),
        "US" | "CA" | "MX" | "PH" | "PR" | "CL" | "CO" | "VE"
    )
}

/// The photo to include: file bytes and extension.
pub struct Photo {
    pub data: Vec<u8>,
    pub ext: String,
}

/// Renders the recipe. `locale` (e.g. "en-US") picks the paper size.
pub fn render(recipe: &Recipe, photo: Option<Photo>, locale: &str) -> AppResult<Vec<u8>> {
    let title = recipe.title.clone();
    render_many(vec![(recipe.clone(), photo)], &title, locale)
}

/// Several recipes in one PDF, each starting on a new page; `title` names the document.
pub fn render_many(
    recipes: Vec<(Recipe, Option<Photo>)>,
    title: &str,
    locale: &str,
) -> AppResult<Vec<u8>> {
    let page = if uses_letter(locale) { LETTER } else { A4 };
    let fonts = Fonts::load()?;
    let mut l = Layout::new(&fonts, page);
    for (recipe, photo) in recipes {
        l.start_recipe(&recipe.title);
        lay_out(&mut l, &recipe, photo);
    }
    l.finish(title)
}

fn lay_out(l: &mut Layout, r: &Recipe, photo: Option<Photo>) {
    l.paragraph(&r.title, Style::new(22.0).bold(), 0.0);
    l.gap(4.0);
    let mut meta = Vec::new();
    if let Some(s) = r.servings {
        meta.push(format!("Serves {s}"));
    }
    if let Some(m) = r.prep_minutes {
        meta.push(format!("Prep {}", minutes(m)));
    }
    if let Some(m) = r.cook_minutes {
        meta.push(format!("Cook {}", minutes(m)));
    }
    if let Some(n) = r.rating {
        meta.push(format!("Rated {n} out of 5"));
    }
    if !meta.is_empty() {
        l.paragraph(&meta.join(" · "), Style::new(10.5).color(MUTED), 0.0);
    }
    let mut labels = Vec::new();
    if !r.categories.is_empty() {
        labels.push(r.categories.join(", "));
    }
    if !r.tags.is_empty() {
        labels.push(
            r.tags
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    if !labels.is_empty() {
        l.paragraph(&labels.join(" · "), Style::new(10.5).color(ACCENT), 0.0);
    }
    if !r.description.trim().is_empty() {
        l.gap(8.0);
        l.paragraph(&r.description, Style::new(11.5), 0.0);
    }
    if let Some(photo) = photo {
        match image(photo) {
            Some(img) => {
                l.gap(14.0);
                l.image(img);
            }
            None => log::warn!("photo not included in the PDF (unsupported format)"),
        }
    }

    l.heading("Ingredients");
    if r.ingredients.is_empty() {
        l.paragraph("None listed.", Style::new(11.0).color(MUTED), 0.0);
    }
    for (i, line) in r.ingredients.iter().enumerate() {
        if line.is_heading {
            if i > 0 {
                l.gap(6.0);
            }
            l.keep_with_next(2);
            l.paragraph(&line.item, Style::new(11.0).bold().color(ACCENT), 0.0);
            continue;
        }
        let amount = [
            line.quantity_text.as_str(),
            line.unit.as_deref().unwrap_or(""),
        ]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
        let mut text = [amount.as_str(), line.item.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        if let Some(note) = line.prep_note.as_deref().filter(|n| !n.is_empty()) {
            text.push_str(", ");
            text.push_str(note);
        }
        l.item("•", &text, Style::new(11.0), 14.0);
    }

    l.heading("Steps");
    if r.steps.is_empty() {
        l.paragraph("None listed.", Style::new(11.0).color(MUTED), 0.0);
    }
    for (i, step) in r.steps.iter().enumerate() {
        if i > 0 {
            l.gap(5.0);
        }
        l.item(&format!("{}.", i + 1), step, Style::new(11.0), 20.0);
    }

    if !r.notes.trim().is_empty() {
        l.heading("Notes");
        l.paragraph(&r.notes, Style::new(11.0), 0.0);
    }

    let n = &r.nutrition;
    let facts: Vec<String> = [
        ("Calories", n.calories, ""),
        ("Protein", n.protein_g, " g"),
        ("Carbs", n.carbs_g, " g"),
        ("Fat", n.fat_g, " g"),
    ]
    .iter()
    .filter_map(|(label, v, unit)| v.map(|v| format!("{label} {}{unit}", number(v))))
    .collect();
    if !facts.is_empty() {
        l.heading("Nutrition per serving");
        l.paragraph(&facts.join(" · "), Style::new(11.0), 0.0);
    }

    if let Some(url) = r.source_url.as_deref().filter(|u| !u.is_empty()) {
        l.heading("Source");
        l.paragraph(url, Style::new(10.0).color(MUTED), 0.0);
    }
}

/// "45 min", "1 h", "1 h 30 min" (same as the app shows).
fn minutes(m: u32) -> String {
    match (m / 60, m % 60) {
        (0, min) => format!("{min} min"),
        (h, 0) => format!("{h} h"),
        (h, min) => format!("{h} h {min} min"),
    }
}

/// 512 → "512", 14.5 → "14.5".
fn number(v: f64) -> String {
    let s = format!("{:.1}", v);
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

fn image(photo: Photo) -> Option<Image> {
    let data = photo.data.into();
    match photo.ext.as_str() {
        "jpg" | "jpeg" => Image::from_jpeg(data, true),
        "png" => Image::from_png(data, true),
        "webp" => Image::from_webp(data, true),
        "gif" => Image::from_gif(data, true),
        _ => return None,
    }
    .inspect_err(|e| log::warn!("photo can't be read for the PDF: {e}"))
    .ok()
}

struct Face {
    font: Font,
    shaper: rustybuzz::Face<'static>,
}

struct Fonts {
    regular: Face,
    bold: Face,
}

impl Fonts {
    fn load() -> AppResult<Self> {
        let face = |data: &'static [u8]| -> AppResult<Face> {
            let broken = || AppError::Io(std::io::Error::other("built-in font unreadable"));
            Ok(Face {
                font: Font::new(data.to_vec().into(), 0).ok_or_else(broken)?,
                shaper: rustybuzz::Face::from_slice(data, 0).ok_or_else(broken)?,
            })
        };
        Ok(Fonts {
            regular: face(REGULAR)?,
            bold: face(BOLD)?,
        })
    }

    fn get(&self, bold: bool) -> &Face {
        if bold { &self.bold } else { &self.regular }
    }

    /// Width of `text` in points at `size`.
    fn width(&self, text: &str, bold: bool, size: f32) -> f32 {
        let face = &self.get(bold).shaper;
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        let shaped = rustybuzz::shape(face, &[], buffer);
        let units: i32 = shaped.glyph_positions().iter().map(|p| p.x_advance).sum();
        units as f32 * size / f32::from(face.units_per_em() as u16)
    }
}

#[derive(Clone, Copy)]
struct Style {
    size: f32,
    bold: bool,
    color: (u8, u8, u8),
}

impl Style {
    fn new(size: f32) -> Self {
        Style {
            size,
            bold: false,
            color: TEXT,
        }
    }

    fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    fn color(mut self, color: (u8, u8, u8)) -> Self {
        self.color = color;
        self
    }

    fn line_height(&self) -> f32 {
        self.size * 1.4
    }
}

enum Op {
    Text {
        x: f32,
        y: f32,
        style: Style,
        text: String,
    },
    Image {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        image: Image,
    },
    Rule {
        x: f32,
        y: f32,
        w: f32,
    },
}

/// Places content top to bottom, starting a new page when the next piece won't fit.
struct Layout<'a> {
    fonts: &'a Fonts,
    page: (f32, f32),
    pages: Vec<Vec<Op>>,
    /// Footer title of each page (the recipe on it).
    titles: Vec<String>,
    /// Top of the next line.
    y: f32,
    /// Space to keep free below a heading so it isn't left alone at a page end.
    keep: f32,
}

impl<'a> Layout<'a> {
    fn new(fonts: &'a Fonts, page: (f32, f32)) -> Self {
        Layout {
            fonts,
            page,
            pages: vec![Vec::new()],
            titles: vec![String::new()],
            y: MARGIN,
            keep: 0.0,
        }
    }

    fn content_width(&self) -> f32 {
        self.page.0 - 2.0 * MARGIN
    }

    fn bottom(&self) -> f32 {
        self.page.1 - MARGIN - FOOTER_GAP
    }

    /// Starts a new page unless `height` (plus anything kept with it) fits.
    fn ensure(&mut self, height: f32) {
        let needed = height + std::mem::take(&mut self.keep);
        if self.y + needed > self.bottom() && self.y > MARGIN {
            self.new_page();
        }
    }

    fn new_page(&mut self) {
        let title = self.titles.last().cloned().unwrap_or_default();
        self.pages.push(Vec::new());
        self.titles.push(title);
        self.y = MARGIN;
    }

    /// Each recipe starts on a fresh page and names the pages it fills.
    fn start_recipe(&mut self, title: &str) {
        if self.pages.last().is_some_and(|p| !p.is_empty()) {
            self.new_page();
        }
        if let Some(t) = self.titles.last_mut() {
            *t = title.to_string();
        }
    }

    fn push(&mut self, op: Op) {
        self.pages.last_mut().expect("a page").push(op);
    }

    fn gap(&mut self, points: f32) {
        if self.y > MARGIN {
            self.y += points;
        }
    }

    /// Keeps the next block on the same page as roughly `lines` lines after it.
    fn keep_with_next(&mut self, lines: usize) {
        self.keep = lines as f32 * Style::new(11.0).line_height();
    }

    fn heading(&mut self, text: &str) {
        self.gap(18.0);
        let style = Style::new(14.0).bold();
        self.keep_with_next(3);
        self.ensure(style.line_height() + 8.0);
        self.line(MARGIN, text, style);
        let (x, y, w) = (MARGIN, self.y + 1.0, self.content_width());
        self.push(Op::Rule { x, y, w });
        self.y += 7.0;
    }

    /// One line of text at the current position (which must already fit).
    fn line(&mut self, x: f32, text: &str, style: Style) {
        // Baseline at about 78% of the line box.
        let y = self.y + style.line_height() * 0.78;
        self.push(Op::Text {
            x,
            y,
            style,
            text: text.to_string(),
        });
        self.y += style.line_height();
    }

    /// Wrapped text; each line of the source starts a new line.
    fn paragraph(&mut self, text: &str, style: Style, indent: f32) {
        let width = self.content_width() - indent;
        for source in text.lines() {
            let source = source.trim();
            if source.is_empty() {
                self.y += style.line_height() * 0.5;
                continue;
            }
            for line in self.wrap(source, style, width) {
                self.ensure(style.line_height());
                self.line(MARGIN + indent, &line, style);
            }
        }
    }

    /// A list entry: `marker` in the margin, `text` wrapped with a hanging indent.
    fn item(&mut self, marker: &str, text: &str, style: Style, indent: f32) {
        let lines = self.wrap(text.trim(), style, self.content_width() - indent);
        for (i, line) in lines.iter().enumerate() {
            self.ensure(style.line_height());
            if i == 0 {
                let top = self.y;
                self.line(MARGIN, marker, style.color(MUTED));
                self.y = top;
            }
            self.line(MARGIN + indent, line, style);
        }
    }

    fn image(&mut self, image: Image) {
        let (w, h) = image.size();
        let scale = (self.content_width() / w as f32).min(MAX_PHOTO_HEIGHT / h as f32);
        let (w, h) = (w as f32 * scale, h as f32 * scale);
        self.ensure(h);
        let (x, y) = (MARGIN, self.y);
        self.push(Op::Image { x, y, w, h, image });
        self.y += h;
    }

    /// Greedy word wrap; words longer than a line (URLs) are split by character.
    fn wrap(&self, text: &str, style: Style, width: f32) -> Vec<String> {
        let fits = |s: &str| self.fonts.width(s, style.bold, style.size) <= width;
        let mut lines = Vec::new();
        let mut current = String::new();
        for word in text.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if fits(&candidate) {
                current = candidate;
                continue;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            if fits(word) {
                current = word.to_string();
            } else {
                for c in word.chars() {
                    current.push(c);
                    if !fits(&current) {
                        current.pop();
                        lines.push(std::mem::take(&mut current));
                        current.push(c);
                    }
                }
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
        lines
    }

    /// Draws every page with a footer ("title · page n of m") and returns the PDF bytes.
    fn finish(self, title: &str) -> AppResult<Vec<u8>> {
        let mut doc = Document::new();
        doc.set_metadata(
            Metadata::new()
                .title(title.to_string())
                .creator(format!("Recipio {}", env!("CARGO_PKG_VERSION"))),
        );
        let count = self.pages.len();
        let footer = Style::new(8.5).color(MUTED);
        let short = |title: &str| -> String {
            let mut t: String = title.chars().take(70).collect();
            if title.chars().count() > 70 {
                t.push('…');
            }
            t
        };
        for (n, (ops, page_title)) in self.pages.into_iter().zip(self.titles).enumerate() {
            let short_title = short(&page_title);
            let settings = PageSettings::from_wh(self.page.0, self.page.1)
                .ok_or_else(|| AppError::Io(std::io::Error::other("bad page size")))?;
            let mut page = doc.start_page_with(settings);
            let mut surface = page.surface();
            let mut ops = ops;
            let label = format!("Page {} of {count}", n + 1);
            let footer_y = self.page.1 - MARGIN + 4.0;
            ops.push(Op::Text {
                x: MARGIN,
                y: footer_y,
                style: footer,
                text: short_title.clone(),
            });
            let label_w = self.fonts.width(&label, false, footer.size);
            ops.push(Op::Text {
                x: self.page.0 - MARGIN - label_w,
                y: footer_y,
                style: footer,
                text: label,
            });
            for op in ops {
                match op {
                    Op::Text { x, y, style, text } => {
                        surface.set_fill(Some(fill(style.color)));
                        surface.draw_text(
                            Point::from_xy(x, y),
                            self.fonts.get(style.bold).font.clone(),
                            style.size,
                            &text,
                            false,
                            TextDirection::Auto,
                        );
                    }
                    Op::Image { x, y, w, h, image } => {
                        if let Some(size) = Size::from_wh(w, h) {
                            surface.push_transform(&Transform::from_translate(x, y));
                            surface.draw_image(image, size);
                            surface.pop();
                        }
                    }
                    Op::Rule { x, y, w } => {
                        let mut path = PathBuilder::new();
                        if let Some(rect) = Rect::from_xywh(x, y, w, 0.75) {
                            path.push_rect(rect);
                        }
                        if let Some(path) = path.finish() {
                            surface.set_fill(Some(fill((227, 222, 214))));
                            surface.draw_path(&path);
                        }
                    }
                }
            }
            surface.finish();
            page.finish();
        }
        doc.finish()
            .map_err(|e| AppError::Io(std::io::Error::other(format!("PDF: {e:?}"))))
    }
}

fn fill((r, g, b): (u8, u8, u8)) -> Fill {
    Fill {
        paint: rgb::Color::new(r, g, b).into(),
        opacity: NormalizedF32::ONE,
        rule: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{IngredientLine, Nutrition, VideoStatus};

    fn recipe(steps: usize) -> Recipe {
        Recipe {
            id: "x".into(),
            title: "Weeknight Chicken Tikka Masala".into(),
            description: "Creamy & mildly spiced — ready in under an hour.".into(),
            servings: Some(4),
            prep_minutes: Some(15),
            cook_minutes: Some(65),
            photo_path: None,
            notes: "Use thighs, not breasts.\nFreezes well.".into(),
            rating: Some(5),
            youtube_url: Some("https://youtu.be/secret-video".into()),
            video_path: Some("videos/x.mp4".into()),
            video_status: VideoStatus::Ready,
            source_url: Some("https://example.com/a/very/long/url/that/needs/to/be/broken/up/because/it/has/no/spaces/at/all/1234567890".into()),
            nutrition: Nutrition { calories: Some(512.0), protein_g: Some(38.0), carbs_g: Some(14.5), fat_g: None },
            ingredients: vec![
                IngredientLine { is_heading: true, quantity: None, quantity_max: None, unit: None, item: "Marinade".into(), prep_note: None, quantity_text: String::new() },
                IngredientLine { is_heading: false, quantity: Some(1.5), quantity_max: None, unit: Some("cups".into()), item: "plain yogurt".into(), prep_note: Some("full fat".into()), quantity_text: "1½".into() },
                IngredientLine { is_heading: false, quantity: None, quantity_max: None, unit: None, item: "salt".into(), prep_note: None, quantity_text: String::new() },
            ],
            steps: (1..=steps).map(|i| format!("Step {i}: stir the sauce gently over a medium-low heat until it thickens and coats the back of a spoon, about 8 minutes.")).collect(),
            categories: vec!["Dinner".into()],
            tags: vec!["easy".into()],
            bookmarks: vec![],
            shared_by: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn renders_a_recipe_and_breaks_pages() {
        let short = render(&recipe(3), None, "en-US").unwrap();
        assert!(short.starts_with(b"%PDF"));
        let long = render(&recipe(60), None, "de-DE").unwrap();
        assert!(long.len() > short.len());
        let text = String::from_utf8_lossy(&long);
        assert!(
            text.matches("/Type /Page").count() >= 3
                || text.contains("/Count 3")
                || text.contains("/Count 4")
        );
    }

    #[test]
    fn lays_out_the_text_but_never_the_video() {
        let fonts = Fonts::load().unwrap();
        let mut l = Layout::new(&fonts, LETTER);
        lay_out(&mut l, &recipe(2), None);
        let text: Vec<String> = l
            .pages
            .iter()
            .flatten()
            .filter_map(|op| match op {
                Op::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        let all = text.join("\n");
        for expected in [
            "Weeknight Chicken Tikka Masala",
            "Serves 4 · Prep 15 min · Cook 1 h 5 min · Rated 5 out of 5",
            "Dinner · #easy",
            "Marinade",
            "1½ cups plain yogurt, full fat",
            "salt",
            "1.",
            "Freezes well.",
            "Calories 512 · Protein 38 g · Carbs 14.5 g",
        ] {
            assert!(all.contains(expected), "missing {expected:?} in:\n{all}");
        }
        assert!(
            !all.contains("youtu") && !all.contains("secret-video"),
            "no video link"
        );
        assert!(!all.to_lowercase().contains("video"));
    }

    #[test]
    fn puts_several_recipes_on_their_own_pages() {
        let fonts = Fonts::load().unwrap();
        let mut l = Layout::new(&fonts, LETTER);
        for title in ["First", "Second"] {
            let mut r = recipe(2);
            r.title = title.into();
            l.start_recipe(&r.title);
            lay_out(&mut l, &r, None);
        }
        assert_eq!(l.pages.len(), l.titles.len());
        let second = l.titles.iter().position(|t| t == "Second").unwrap();
        assert!(second > 0 && l.titles[..second].iter().all(|t| t == "First"));
        let pdf = render_many(vec![(recipe(1), None), (recipe(1), None)], "Two", "en-US").unwrap();
        assert!(pdf.starts_with(b"%PDF"));
    }

    #[test]
    fn picks_paper_and_formats() {
        assert!(uses_letter("en-US"));
        assert!(uses_letter("es_MX"));
        assert!(!uses_letter("en-GB"));
        assert!(!uses_letter("de"));
        assert_eq!(minutes(65), "1 h 5 min");
        assert_eq!(minutes(45), "45 min");
        assert_eq!(number(14.5), "14.5");
        assert_eq!(number(512.0), "512");
    }

    #[test]
    fn wraps_text_to_width() {
        let fonts = Fonts::load().unwrap();
        let l = Layout::new(&fonts, LETTER);
        let style = Style::new(11.0);
        let lines = l.wrap(&"word ".repeat(200), style, 300.0);
        assert!(lines.len() > 5);
        assert!(
            lines
                .iter()
                .all(|line| fonts.width(line, false, 11.0) <= 300.0)
        );
        let url = l.wrap(&"x".repeat(500), style, 300.0);
        assert!(
            url.len() > 1
                && url
                    .iter()
                    .all(|line| fonts.width(line, false, 11.0) <= 300.0)
        );
    }
}

#[cfg(test)]
mod live {
    /// Renders a real recipe: `RECIPIO_PDF_DB=<db copy> RECIPIO_PDF_MEDIA=<media dir>
    /// RECIPIO_PDF_ID=<id> RECIPIO_PDF_OUT=<file> cargo test -- --ignored live_pdf`.
    #[test]
    #[ignore]
    fn live_pdf() {
        let var = |k| std::env::var(k).unwrap();
        let conn = crate::db::open(std::path::Path::new(&var("RECIPIO_PDF_DB"))).unwrap();
        let recipe = crate::db::recipes::get(&conn, &var("RECIPIO_PDF_ID")).unwrap();
        let photo = recipe.photo_path.as_ref().map(|rel| {
            let file = std::path::Path::new(&var("RECIPIO_PDF_MEDIA")).join(rel);
            super::Photo {
                ext: file.extension().unwrap().to_str().unwrap().to_string(),
                data: std::fs::read(file).unwrap(),
            }
        });
        let pdf = super::render(&recipe, photo, "en-US").unwrap();
        std::fs::write(var("RECIPIO_PDF_OUT"), pdf).unwrap();
    }
}
