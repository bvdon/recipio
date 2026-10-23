//! Reads a schema.org `Recipe` from a web page's JSON-LD blocks
//! (`<script type="application/ld+json">`). Pure: the page is fetched in `web_import`.
//!
//! Handles what recipe sites actually publish: several blocks per page, `@graph` arrays,
//! `@id` references, `@type` as a string or list, instructions as text, `HowToStep`s or
//! `HowToSection`s, images as a string, object or list, yield as a string or number, and
//! HTML markup or entities inside text values.

use std::collections::HashMap;

use reqwest::Url;
use scraper::{Html, Selector};
use serde_json::{Map, Value};

use crate::duration;
use crate::models::Nutrition;

/// Limits so a hostile or broken page can't produce an unsaveable recipe.
const MAX_LINES: usize = 200;
const MAX_IMAGES: usize = 5;
/// Nesting depth searched for the recipe and followed through references.
const MAX_DEPTH: usize = 12;

/// The recipe's fields, cleaned to plain text. Ingredient lines are still unparsed.
#[derive(Debug, Default, PartialEq)]
pub struct Extracted {
    pub title: String,
    pub description: String,
    pub servings: Option<u32>,
    pub prep_minutes: Option<u32>,
    pub cook_minutes: Option<u32>,
    pub ingredients: Vec<String>,
    pub steps: Vec<String>,
    pub nutrition: Nutrition,
    /// `recipeCategory`, `recipeCuisine` and `keywords`, one entry each.
    pub labels: Vec<String>,
    /// Absolute http(s) URLs, best first.
    pub images: Vec<String>,
    /// Every URL given for the recipe's videos (any host).
    pub videos: Vec<String>,
}

/// Finds the page's recipe; `None` when it has no schema.org Recipe data.
pub fn extract(html: &str, base: &Url) -> Option<Extracted> {
    let blocks = json_blocks(html);
    let mut index = HashMap::new();
    for block in &blocks {
        index_ids(block, &mut index, 0);
    }
    let mut recipes = Vec::new();
    for block in &blocks {
        find_recipes(block, &mut recipes, 0);
    }
    // Some pages also describe related recipes; the main one has ingredients.
    let recipe = recipes
        .iter()
        .find(|r| r.get("recipeIngredient").is_some_and(|v| !is_empty(v)))
        .or(recipes.first())?;
    Some(read_recipe(recipe, &Refs(&index), base))
}

/// Parsed JSON of every JSON-LD script block; broken blocks are skipped.
fn json_blocks(html: &str) -> Vec<Value> {
    let doc = Html::parse_document(html);
    let selector = Selector::parse("script[type]").expect("valid selector");
    doc.select(&selector)
        .filter(|el| {
            el.attr("type").is_some_and(|t| {
                t.trim()
                    .to_ascii_lowercase()
                    .starts_with("application/ld+json")
            })
        })
        .filter_map(|el| parse_json(&el.text().collect::<String>()))
        .collect()
}

fn parse_json(raw: &str) -> Option<Value> {
    let mut s = raw.trim();
    for (open, close) in [
        ("<!--", "-->"),
        ("//<![CDATA[", "//]]>"),
        ("<![CDATA[", "]]>"),
    ] {
        if let Some(inner) = s.strip_prefix(open) {
            s = inner.trim_end().strip_suffix(close).unwrap_or(inner).trim();
        }
    }
    serde_json::from_str(s)
        .or_else(|_| serde_json::from_str(&escape_controls(s)))
        .inspect_err(|e| log::info!("skipping unreadable JSON-LD block: {e}"))
        .ok()
}

/// Raw line breaks and tabs inside JSON strings are invalid but common; escape them so
/// the line breaks survive (they separate instruction steps).
fn escape_controls(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let (mut in_string, mut escaped) = (false, false);
    for c in s.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            } else if c == '\n' {
                out.push_str("\\n");
                continue;
            } else if c.is_control() {
                out.push(' ');
                continue;
            }
        } else if c == '"' {
            in_string = true;
        }
        out.push(c);
    }
    out
}

/// Objects with an `@id`, so references like `{"@id": "#primaryimage"}` can be followed.
fn index_ids<'a>(v: &'a Value, index: &mut HashMap<&'a str, &'a Map<String, Value>>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    match v {
        Value::Array(items) => items.iter().for_each(|x| index_ids(x, index, depth + 1)),
        Value::Object(o) => {
            if let Some(id) = o.get("@id").and_then(Value::as_str)
                && o.len() > 1
            {
                index.entry(id).or_insert(o);
            }
            o.values().for_each(|x| index_ids(x, index, depth + 1));
        }
        _ => {}
    }
}

struct Refs<'a>(&'a HashMap<&'a str, &'a Map<String, Value>>);

impl<'a> Refs<'a> {
    /// The object a bare `{"@id": …}` reference points to, or the object itself.
    fn resolve(&self, o: &'a Map<String, Value>) -> &'a Map<String, Value> {
        match o.get("@id").and_then(Value::as_str) {
            Some(id) if o.len() == 1 => self.0.get(id).copied().unwrap_or(o),
            _ => o,
        }
    }
}

fn find_recipes<'a>(v: &'a Value, out: &mut Vec<&'a Map<String, Value>>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    match v {
        Value::Array(items) => items.iter().for_each(|x| find_recipes(x, out, depth + 1)),
        Value::Object(o) => {
            if has_type(o, "recipe") {
                out.push(o);
            }
            o.values().for_each(|x| find_recipes(x, out, depth + 1));
        }
        _ => {}
    }
}

/// Whether `@type` names `name` (lowercase), accepting `Recipe`, `schema:Recipe` and
/// `http://schema.org/Recipe`.
fn has_type(o: &Map<String, Value>, name: &str) -> bool {
    let matches = |t: &Value| {
        t.as_str()
            .and_then(|s| s.rsplit(['/', ':', '#']).next())
            .is_some_and(|s| s.eq_ignore_ascii_case(name))
    };
    match o.get("@type") {
        Some(Value::Array(types)) => types.iter().any(matches),
        Some(t) => matches(t),
        None => false,
    }
}

fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

fn read_recipe(r: &Map<String, Value>, refs: &Refs, base: &Url) -> Extracted {
    let text = |key: &str| r.get(key).and_then(first_text).unwrap_or_default();
    let time = |key: &str| {
        r.get(key)
            .and_then(first_text)
            .and_then(|t| duration::minutes(&t))
    };

    let prep = time("prepTime");
    let cook = time("cookTime").or_else(|| {
        // Only a total: the cooking part is what's left after prep.
        let total = time("totalTime")?;
        total.checked_sub(prep?).filter(|&m| m > 0)
    });

    let mut ingredients = Vec::new();
    if let Some(v) = r.get("recipeIngredient").or(r.get("ingredients")) {
        collect_lines(v, &mut ingredients);
    }
    let mut steps = Vec::new();
    if let Some(v) = r.get("recipeInstructions") {
        collect_steps(v, refs, &mut steps, 0);
    }
    for list in [&mut ingredients, &mut steps] {
        list.truncate(MAX_LINES);
    }

    let mut labels = Vec::new();
    for key in ["recipeCategory", "recipeCuisine", "keywords"] {
        if let Some(v) = r.get(key) {
            collect_labels(v, &mut labels);
        }
    }

    let mut images = Vec::new();
    if let Some(v) = r.get("image") {
        collect_images(v, refs, base, &mut images, 0);
    }
    // Largest first when sizes are given; otherwise the page's order.
    images.sort_by_key(|&(_, area)| std::cmp::Reverse(area));
    let mut seen = Vec::<String>::new();
    for (url, _) in images {
        if !seen.contains(&url) && seen.len() < MAX_IMAGES {
            seen.push(url);
        }
    }

    let mut videos = Vec::new();
    if let Some(v) = r.get("video") {
        collect_videos(v, refs, &mut videos, 0);
    }

    Extracted {
        title: one_line(&text("name")),
        description: plain_lines(&text("description")).join("\n"),
        servings: r.get("recipeYield").and_then(servings),
        prep_minutes: prep,
        cook_minutes: cook,
        ingredients,
        steps,
        nutrition: r
            .get("nutrition")
            .and_then(Value::as_object)
            .map(|o| read_nutrition(refs.resolve(o)))
            .unwrap_or_default(),
        labels,
        images: seen,
        videos,
    }
}

/// First usable text of a value: a string, number, or the first such item of a list.
fn first_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(html_text(s)).filter(|t| !t.trim().is_empty()),
        Value::Number(n) => Some(n.to_string()),
        Value::Array(items) => items.iter().find_map(first_text),
        Value::Object(o) => o.get("@value").and_then(first_text),
        _ => None,
    }
}

/// Text lines of a string or list (ingredients), one per entry or line.
fn collect_lines(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.extend(text_lines(s)),
        Value::Array(items) => items.iter().for_each(|x| collect_lines(x, out)),
        Value::Object(o) => {
            if let Some(t) = o.get("text").or(o.get("name")) {
                collect_lines(t, out);
            }
        }
        _ => {}
    }
}

fn collect_steps(v: &Value, refs: &Refs, out: &mut Vec<String>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    match v {
        Value::String(s) => out.extend(text_lines(s).map(|l| strip_step_number(&l))),
        Value::Array(items) => items
            .iter()
            .for_each(|x| collect_steps(x, refs, out, depth + 1)),
        Value::Object(o) => {
            let o = refs.resolve(o);
            let text = o.get("text").filter(|t| !is_empty(t));
            match (text, o.get("itemListElement")) {
                // A HowToSection (or ItemList): its name goes in front of its first step.
                (None, Some(items)) => {
                    let start = out.len();
                    collect_steps(items, refs, out, depth + 1);
                    let name = o.get("name").and_then(first_text).map(|n| one_line(&n));
                    if let Some(name) = name.filter(|n| !n.is_empty())
                        && let Some(first) = out.get_mut(start)
                    {
                        *first = format!("{}: {first}", name.trim_end_matches(':'));
                    }
                }
                // HowToStep / HowToDirection / HowToTip; some sites only fill `name`.
                (Some(t), _) => collect_steps(t, refs, out, depth + 1),
                (None, None) => {
                    if let Some(name) = o.get("name") {
                        collect_steps(name, refs, out, depth + 1);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Category/cuisine/keyword names: lists, or comma-separated strings.
fn collect_labels(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            for part in html_text(s).split(',') {
                let name = one_line(part);
                if !name.is_empty() && !out.iter().any(|l| l.eq_ignore_ascii_case(&name)) {
                    out.push(name);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|x| collect_labels(x, out)),
        Value::Object(o) => {
            if let Some(name) = o.get("name") {
                collect_labels(name, out);
            }
        }
        _ => {}
    }
}

/// Image URLs with their pixel area (0 when unknown).
fn collect_images(v: &Value, refs: &Refs, base: &Url, out: &mut Vec<(String, u64)>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    match v {
        Value::String(s) => {
            if let Some(url) = absolute(s, base) {
                out.push((url, 0));
            }
        }
        Value::Array(items) => items
            .iter()
            .for_each(|x| collect_images(x, refs, base, out, depth + 1)),
        Value::Object(o) => {
            let o = refs.resolve(o);
            let url = o.get("url").or(o.get("contentUrl")).and_then(Value::as_str);
            if let Some(url) = url.and_then(|u| absolute(u, base)) {
                let size = |k: &str| o.get(k).and_then(pixels).unwrap_or(0);
                out.push((url, size("width") * size("height")));
            }
        }
        _ => {}
    }
}

fn collect_videos(v: &Value, refs: &Refs, out: &mut Vec<String>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    match v {
        Value::String(s) => out.push(s.trim().to_string()),
        Value::Array(items) => items
            .iter()
            .for_each(|x| collect_videos(x, refs, out, depth + 1)),
        Value::Object(o) => {
            let o = refs.resolve(o);
            for key in ["contentUrl", "embedUrl", "url"] {
                if let Some(url) = o.get(key).and_then(Value::as_str) {
                    out.push(url.trim().to_string());
                }
            }
        }
        _ => {}
    }
}

/// Width/height as a number, numeric string or `QuantitativeValue`.
fn pixels(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f as u64)),
        Value::String(s) => s.trim().trim_end_matches("px").parse().ok(),
        Value::Object(o) => o.get("value").and_then(pixels),
        _ => None,
    }
}

fn absolute(url: &str, base: &Url) -> Option<String> {
    let url = base.join(url.trim()).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

/// Servings from `recipeYield`: `4`, `"4"`, `"4-6 servings"`, `["6", "6 servings"]`.
fn servings(v: &Value) -> Option<u32> {
    let n = match v {
        Value::Number(n) => n.as_f64()?.round() as u32,
        Value::String(s) => {
            let digits: String = s
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse().ok()?
        }
        Value::Array(items) => return items.iter().find_map(servings),
        _ => return None,
    };
    (1..=1000).contains(&n).then_some(n)
}

fn read_nutrition(o: &Map<String, Value>) -> Nutrition {
    let amount = |key: &str| o.get(key).and_then(number);
    Nutrition {
        calories: amount("calories"),
        protein_g: amount("proteinContent"),
        carbs_g: amount("carbohydrateContent"),
        fat_g: amount("fatContent"),
    }
}

/// The leading number of `"240 kcal"`, `"12.5 g"`, `"1,200 calories"`, `"3,5 g"`.
fn number(v: &Value) -> Option<f64> {
    let n = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => {
            let token: String = s
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
                .collect();
            let thousands = token
                .rsplit_once(',')
                .is_some_and(|(_, tail)| tail.len() == 3 && !token.contains('.'));
            let token = if thousands {
                token.replace(',', "")
            } else {
                token.replace(',', ".")
            };
            token.parse().ok()?
        }
        _ => return None,
    };
    (n.is_finite() && n >= 0.0).then_some(n)
}

/// Plain text of a value that may contain HTML tags or entities. Block-level tags
/// (`<p>`, `<li>`, `<br>`) become line breaks.
fn html_text(s: &str) -> String {
    if !s.contains(['<', '&']) {
        return s.to_string();
    }
    let mut marked = String::with_capacity(s.len() + 16);
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        marked.push_str(&rest[..i]);
        let tag = rest[i + 1..].trim_start_matches('/');
        let name: String = tag
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "br" | "p"
                | "li"
                | "div"
                | "ol"
                | "ul"
                | "tr"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
        ) {
            marked.push('\n');
        }
        marked.push('<');
        rest = &rest[i + 1..];
    }
    marked.push_str(rest);
    Html::parse_fragment(&marked)
        .root_element()
        .text()
        .collect()
}

/// Non-empty lines of a text value that may contain HTML.
fn text_lines(s: &str) -> impl Iterator<Item = String> {
    plain_lines(&html_text(s)).into_iter()
}

/// Non-empty lines of plain text, each with whitespace collapsed.
fn plain_lines(s: &str) -> Vec<String> {
    s.lines().map(one_line).filter(|l| !l.is_empty()).collect()
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "1. Preheat", "2) Mix", "Step 3: Bake" → the instruction alone (the app numbers steps).
fn strip_step_number(line: &str) -> String {
    let mut rest = line;
    if line
        .get(..5)
        .is_some_and(|p| p.eq_ignore_ascii_case("step "))
    {
        rest = &line[5..];
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 2 {
        return line.to_string();
    }
    match rest[digits..].strip_prefix(['.', ')', ':']) {
        Some(after) if after.starts_with(' ') => after.trim_start().to_string(),
        _ if rest.len() != line.len() && rest[digits..].starts_with(' ') => {
            rest[digits..].trim_start().to_string()
        }
        _ => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://example.com/recipes/pancakes/").unwrap()
    }

    fn page(json: &str) -> String {
        format!(
            r#"<html><head><script type="application/ld+json">{json}</script></head><body></body></html>"#
        )
    }

    fn get(html: &str) -> Extracted {
        extract(html, &base()).expect("recipe found")
    }

    #[test]
    fn reads_a_wordpress_recipe_plugin_page() {
        let r = get(include_str!("../tests/fixtures/wprm_graph.html"));
        assert_eq!(r.title, "Weeknight Chicken Tikka Masala");
        assert_eq!(
            r.description,
            "Creamy, smoky & mildly spiced — ready in under an hour."
        );
        assert_eq!(r.servings, Some(4));
        assert_eq!(r.prep_minutes, Some(15));
        assert_eq!(r.cook_minutes, Some(35));
        assert_eq!(r.ingredients.len(), 9);
        assert_eq!(
            r.ingredients[0],
            "1 ½ lb boneless chicken thighs (cut into bite-size pieces)"
        );
        assert_eq!(r.ingredients[4], "1 (14 oz) can crushed tomatoes");
        assert_eq!(
            r.steps,
            vec![
                "For the marinade: Mix the yogurt, lemon juice, garam masala and salt in a bowl.",
                "Add the chicken, stir to coat, and leave for 15 minutes.",
                "For the sauce: Melt the butter in a large pan over medium heat.",
                "Add the onion and cook until soft, about 8 minutes.",
                "Stir in the tomatoes and cream, add the chicken and simmer for 20 minutes.",
            ]
        );
        assert_eq!(
            r.images,
            vec![
                "https://www.example-wp.com/wp-content/uploads/2026/03/tikka-masala.jpg",
                "https://www.example-wp.com/wp-content/uploads/2026/03/tikka-masala-500x375.jpg",
                "https://www.example-wp.com/wp-content/uploads/2026/03/tikka-masala-500x500.jpg",
            ],
            "biggest first, then page order"
        );
        assert_eq!(r.videos, vec!["https://www.youtube.com/embed/dQw4w9WgXcQ"]);
        assert_eq!(r.nutrition.calories, Some(512.0));
        assert_eq!(r.nutrition.protein_g, Some(38.0));
        assert_eq!(r.nutrition.carbs_g, Some(14.5));
        assert_eq!(r.nutrition.fat_g, Some(33.0));
        assert_eq!(
            r.labels,
            vec![
                "Main Course",
                "Indian",
                "chicken curry",
                "tikka masala",
                "weeknight"
            ]
        );
    }

    #[test]
    fn reads_a_top_level_array_with_html_in_steps() {
        let r = get(include_str!("../tests/fixtures/news_array.html"));
        assert_eq!(r.title, "Classic Banana Bread");
        assert_eq!(r.description, "Moist, tender and full of banana flavor.");
        assert_eq!(r.servings, Some(10));
        assert_eq!(r.prep_minutes, Some(15));
        assert_eq!(r.cook_minutes, Some(65), "total 1h20 minus 15 prep");
        assert_eq!(r.ingredients[0], "2 cups all-purpose flour");
        assert_eq!(r.ingredients[5], "2 large eggs, beaten");
        assert_eq!(
            r.steps,
            vec![
                "Preheat the oven to 350°F (175°C). Grease a 9x5-inch loaf pan.",
                "Mix flour, baking soda and salt in a large bowl.",
                "Stir the bananas, sugar, eggs & melted butter together; fold into the flour \"just until\" combined.",
                "Bake for 60 to 65 minutes.",
            ]
        );
        assert_eq!(
            r.images,
            vec!["https://www.example-news.com/thmb/banana-bread-1500x1125.jpg"]
        );
        assert_eq!(r.nutrition.calories, Some(1200.0), "thousands separator");
        assert!(r.videos.is_empty());
    }

    #[test]
    fn reads_the_recipe_among_several_blocks() {
        let r = get(include_str!("../tests/fixtures/multi_block.html"));
        assert_eq!(r.title, "Spring Pea Risotto");
        assert_eq!(r.servings, Some(4), "\"4 to 6 servings\"");
        assert_eq!(r.prep_minutes, None);
        assert_eq!(r.cook_minutes, Some(40));
        assert_eq!(r.ingredients.len(), 6);
        assert_eq!(
            r.steps,
            vec![
                "Warm the stock in a saucepan and keep it at a bare simmer.",
                "Cook the shallot in butter until translucent, then add the rice and toast for 2 minutes.",
                "Add the stock a ladle at a time, stirring, for about 25 minutes.",
                "Stir in the peas, parmesan and lemon zest. Season and serve.",
            ]
        );
        assert_eq!(
            r.images,
            vec![
                "https://static.example-times.com/images/risotto-superJumbo.jpg",
                "https://static.example-times.com/images/risotto-threeByTwo.jpg",
            ]
        );
        assert_eq!(
            r.labels,
            vec!["dinner", "Italian", "risotto", "peas", "vegetarian"]
        );
    }

    #[test]
    fn reads_plain_text_instructions_and_repairs_broken_json() {
        let r = get(include_str!("../tests/fixtures/plain_text.html"));
        assert_eq!(r.title, "Grandma's Shortbread");
        assert_eq!(r.servings, Some(24));
        assert_eq!(r.prep_minutes, Some(20), "plain-text time");
        assert_eq!(r.cook_minutes, Some(25));
        assert_eq!(
            r.ingredients,
            vec!["1 cup butter, softened", "½ cup sugar", "2 cups flour"]
        );
        assert_eq!(
            r.steps,
            vec![
                "Cream the butter and sugar.",
                "Work in the flour to a smooth dough.",
                "Press into a pan, prick with a fork and bake at 325°F for 25 minutes.",
            ]
        );
        assert_eq!(
            r.images,
            vec!["https://example.com/images/shortbread.jpg"],
            "relative URL resolved against the page"
        );
    }

    #[test]
    fn finds_nothing_on_a_page_without_a_recipe() {
        let html = include_str!("../tests/fixtures/no_recipe.html");
        assert_eq!(extract(html, &base()), None);
        assert_eq!(extract("", &base()), None);
        assert_eq!(extract("<p>not even close</p>", &base()), None);
    }

    #[test]
    fn accepts_type_variants_and_references() {
        let html = page(
            r##"{"@context":"https://schema.org","@graph":[
                {"@type":"ImageObject","@id":"#img","url":"/pics/a.png","width":800,"height":600},
                {"@type":["http://schema.org/Recipe"],"name":"Toast","image":{"@id":"#img"},
                 "recipeIngredient":"1 slice bread\n1 tsp butter","recipeYield":2,
                 "recipeInstructions":[{"@type":"HowToStep","name":"Toast the bread."}]}
            ]}"##,
        );
        let r = get(&html);
        assert_eq!(r.title, "Toast");
        assert_eq!(r.ingredients, vec!["1 slice bread", "1 tsp butter"]);
        assert_eq!(r.servings, Some(2));
        assert_eq!(r.steps, vec!["Toast the bread."]);
        assert_eq!(r.images, vec!["https://example.com/pics/a.png"]);
    }

    #[test]
    fn prefers_the_recipe_with_ingredients() {
        let html = page(
            r#"[{"@type":"Recipe","name":"Related teaser"},
                {"@type":"Recipe","name":"The real one","recipeIngredient":["1 egg"]}]"#,
        );
        assert_eq!(get(&html).title, "The real one");
    }

    #[test]
    fn ignores_unsafe_image_urls_and_caps_lists() {
        let many: Vec<String> = (0..300).map(|i| format!("\"{i} g flour\"")).collect();
        let html = page(&format!(
            r#"{{"@type":"Recipe","name":"Big","image":["javascript:alert(1)","data:image/png;base64,AA","ftp://x/y.jpg"],
                "recipeIngredient":[{}]}}"#,
            many.join(",")
        ));
        let r = get(&html);
        assert!(r.images.is_empty());
        assert_eq!(r.ingredients.len(), MAX_LINES);
    }

    #[test]
    fn converts_html_to_text() {
        assert_eq!(html_text("plain"), "plain");
        assert_eq!(html_text("Salt &amp; pepper"), "Salt & pepper");
        assert_eq!(html_text("It&#39;s &quot;done&quot;"), "It's \"done\"");
        assert_eq!(
            html_text("<p>One</p><p>Two<br/>Three</p>")
                .lines()
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>(),
            vec!["One", "Two", "Three"]
        );
        assert_eq!(html_text("<strong>Bold</strong> move"), "Bold move");
        assert_eq!(html_text("cook < 5 min"), "cook < 5 min");
    }

    #[test]
    fn strips_step_numbers() {
        assert_eq!(strip_step_number("1. Preheat the oven"), "Preheat the oven");
        assert_eq!(strip_step_number("12) Serve"), "Serve");
        assert_eq!(strip_step_number("Step 3: Bake"), "Bake");
        assert_eq!(strip_step_number("Step 4 Rest"), "Rest");
        assert_eq!(
            strip_step_number("350 degrees is hot"),
            "350 degrees is hot"
        );
        assert_eq!(strip_step_number("2 eggs go in"), "2 eggs go in");
        assert_eq!(strip_step_number("1.5 cups"), "1.5 cups");
    }

    #[test]
    fn reads_yields_and_amounts() {
        use serde_json::json;
        assert_eq!(servings(&json!(4)), Some(4));
        assert_eq!(servings(&json!("Makes 12 muffins")), Some(12));
        assert_eq!(servings(&json!(["", "6 servings"])), Some(6));
        assert_eq!(servings(&json!("a crowd")), None);
        assert_eq!(servings(&json!(0)), None);
        assert_eq!(number(&json!("240 kcal")), Some(240.0));
        assert_eq!(number(&json!("3,5 g")), Some(3.5));
        assert_eq!(number(&json!("1,200 calories")), Some(1200.0));
        assert_eq!(number(&json!(12.5)), Some(12.5));
        assert_eq!(number(&json!("n/a")), None);
    }
}
