//! Heuristic parser for pasted ingredient text ("1 1/2 cups flour, sifted"), one line at
//! a time. Used by paste-to-parse in the editor and, later, by web import.
//!
//! It never fails: anything it can't pick apart ends up in `item`, and the owner reviews
//! the result before it is applied.

use crate::models::ParsedIngredient;
use crate::quantity;

/// Units recognised after a quantity, lowercase, every accepted spelling listed.
/// Size words (small, large) and containers people describe loosely stay in the item.
const UNITS: &[&str] = &[
    "cup",
    "cups",
    "c", //
    "tablespoon",
    "tablespoons",
    "tbsp",
    "tbsps",
    "tbs",
    "tbl",
    "t", //
    "teaspoon",
    "teaspoons",
    "tsp",
    "tsps", //
    "ounce",
    "ounces",
    "oz",
    "pound",
    "pounds",
    "lb",
    "lbs", //
    "gram",
    "grams",
    "g",
    "kilogram",
    "kilograms",
    "kg",
    "mg", //
    "milliliter",
    "milliliters",
    "millilitre",
    "millilitres",
    "ml", //
    "liter",
    "liters",
    "litre",
    "litres",
    "l",
    "dl",
    "cl", //
    "pint",
    "pints",
    "pt",
    "quart",
    "quarts",
    "qt",
    "gallon",
    "gallons",
    "gal", //
    "pinch",
    "pinches",
    "dash",
    "dashes",
    "drop",
    "drops",
    "splash",
    "handful",
    "handfuls",
    "clove",
    "cloves",
    "can",
    "cans",
    "tin",
    "tins",
    "jar",
    "jars",
    "bottle",
    "bottles", //
    "package",
    "packages",
    "pkg",
    "packet",
    "packets",
    "box",
    "boxes",
    "bag",
    "bags",
    "envelope",
    "envelopes",
    "stick",
    "sticks",
    "slice",
    "slices",
    "piece",
    "pieces",
    "pc",
    "pcs",
    "bunch",
    "bunches",
    "sprig",
    "sprigs",
    "stalk",
    "stalks",
    "head",
    "heads", //
    "leaf",
    "leaves",
    "sheet",
    "sheets",
    "strip",
    "strips",
    "cube",
    "cubes",
    "knob",
    "scoop",
    "scoops",
    "inch",
    "inches",
    "cm",
];

const TWO_WORD_UNITS: &[&str] = &["fl oz", "fluid ounce", "fluid ounces"];

/// List markers stripped from the start of a line.
const BULLETS: &[char] = &['•', '·', '*', '▢', '□', '☐', '◦', '▪', '✓', '✔'];

/// Parses a pasted block, one ingredient (or heading) per non-blank line.
pub fn parse_block(text: &str) -> Vec<ParsedIngredient> {
    text.lines().filter_map(parse_line).collect()
}

/// Parses one line. `None` if it is blank once list markers are removed.
pub fn parse_line(raw: &str) -> Option<ParsedIngredient> {
    let mut line = raw.trim().trim_start_matches(BULLETS).trim_start();
    for dash in ["- ", "– ", "— "] {
        line = line.strip_prefix(dash).unwrap_or(line);
    }
    let hashes = line.trim_start_matches('#');
    let markdown_heading = hashes.len() != line.len();
    let line = hashes.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return None;
    }

    if markdown_heading || is_heading(&line) {
        return Some(ParsedIngredient {
            is_heading: true,
            item: line.trim_end_matches(':').trim_end().to_string(),
            ..Default::default()
        });
    }

    let tokens = split_glued_units(&line);
    let mut rest: &[String] = &tokens;

    let mut quantity = String::new();
    if let Some((text, used)) = leading_quantity(rest) {
        quantity = text;
        rest = &rest[used..];
    } else if rest.len() > 1 && matches!(rest[0].to_lowercase().as_str(), "a" | "an") {
        // "a pinch of salt": only when a unit follows, so "a few sprigs" stays as typed.
        if unit_len(&rest[1..], true) > 0 {
            quantity = "1".into();
            rest = &rest[1..];
        }
    }

    let mut unit = String::new();
    let n = unit_len(rest, !quantity.is_empty());
    if n > 0 {
        unit = rest[..n].join(" ").trim_end_matches('.').to_string();
        rest = &rest[n..];
    }
    if rest.first().is_some_and(|t| t.eq_ignore_ascii_case("of")) {
        rest = &rest[1..];
    }

    let (mut item, prep_note) = split_prep_note(&rest.join(" "));
    // "2 cloves" or "1 pinch": the unit word was really the ingredient.
    if item.is_empty() {
        item = std::mem::take(&mut unit);
    }
    Some(ParsedIngredient {
        is_heading: false,
        quantity,
        unit,
        item,
        prep_note,
    })
}

/// "For the sauce:", "SAUCE", "For the topping".
fn is_heading(line: &str) -> bool {
    if line.ends_with(':') {
        return true;
    }
    if line.chars().any(|c| c.is_ascii_digit() || is_glyph(c)) {
        return false;
    }
    let letters: Vec<char> = line.chars().filter(|c| c.is_alphabetic()).collect();
    let shouting = letters.len() >= 2 && letters.iter().all(|c| c.is_uppercase());
    let for_the = line.to_lowercase().starts_with("for ")
        && !line.contains(',')
        && line.split(' ').count() <= 6;
    shouting || for_the
}

fn is_glyph(c: char) -> bool {
    matches!(c, '½' | '⅓' | '⅔' | '¼' | '¾' | '⅕'..='⅞' | '⅐' | '⅑' | '⅒')
}

/// Splits on whitespace, and splits a number glued to a word in the first tokens
/// ("200g" → "200", "g"; "1½cups" → "1½", "cups").
fn split_glued_units(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for (i, word) in line.split(' ').enumerate() {
        let starts_numeric = word
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit() || is_glyph(c));
        let split = word
            .char_indices()
            .find(|&(_, c)| c.is_alphabetic() && !is_glyph(c))
            .map(|(at, _)| at);
        // Only a plain number before the letters: "4-inch" and "15-ounce" stay whole.
        let numeric_prefix = |at: usize| {
            word[..at]
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '/') || is_glyph(c))
        };
        match split {
            Some(at) if i < 3 && starts_numeric && numeric_prefix(at) => {
                tokens.push(word[..at].to_string());
                tokens.push(word[at..].to_string());
            }
            _ => tokens.push(word.to_string()),
        }
    }
    tokens
}

/// The longest run of leading tokens that is a quantity ("1 1/2", "2 to 3"), formatted
/// the way the editor shows it, and how many tokens it used.
fn leading_quantity(tokens: &[String]) -> Option<(String, usize)> {
    let first = tokens.first()?.chars().next()?;
    if !(first.is_ascii_digit() || first == '.' || is_glyph(first)) {
        return None;
    }
    (1..=tokens.len().min(4)).rev().find_map(|k| {
        let q = quantity::parse(&tokens[..k].join(" ")).ok()??;
        Some((quantity::format(Some(q.min), q.max), k))
    })
}

/// How many leading tokens form a unit: `cups`, `fl oz`, or `(14 oz) can`. Without a
/// quantity a unit only counts when followed by "of" ("pinch of salt"), so an item
/// that happens to start with a unit word ("Can opener") is left alone.
fn unit_len(tokens: &[String], has_quantity: bool) -> usize {
    let word = |t: &String| t.trim_end_matches(['.', ',']).to_lowercase();
    let n = if tokens.first().is_some_and(|t| t.starts_with('(')) {
        // A size in parentheses only counts as part of the unit if a unit follows it.
        match tokens.iter().position(|t| t.ends_with(')')) {
            Some(close)
                if tokens
                    .get(close + 1)
                    .is_some_and(|t| UNITS.contains(&word(t).as_str())) =>
            {
                close + 2
            }
            _ => 0,
        }
    } else if tokens.len() >= 2
        && TWO_WORD_UNITS.contains(&format!("{} {}", word(&tokens[0]), word(&tokens[1])).as_str())
    {
        2
    } else if tokens
        .first()
        .is_some_and(|t| UNITS.contains(&word(t).as_str()))
    {
        1
    } else {
        0
    };
    // Something has to be left over to be the ingredient, except after a quantity
    // ("2 cloves" is handled by the caller).
    let followed_by_of = tokens.get(n).is_some_and(|t| t.eq_ignore_ascii_case("of"));
    if n > 0 && (has_quantity || followed_by_of) {
        n
    } else {
        0
    }
}

/// "flour, sifted" → ("flour", "sifted"); "onion (finely chopped)" → ("onion", "finely chopped").
fn split_prep_note(text: &str) -> (String, String) {
    if let Some((item, note)) = text.split_once(',') {
        return (item.trim().to_string(), note.trim().to_string());
    }
    if let Some(inner) = text.strip_suffix(')')
        && let Some(open) = inner.rfind('(')
        && open > 0
    {
        return (
            inner[..open].trim().to_string(),
            inner[open + 1..].trim().to_string(),
        );
    }
    (text.trim().to_string(), String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: &str) -> (String, String, String, String) {
        let r = parse_line(line).unwrap();
        assert!(!r.is_heading, "{line}");
        (r.quantity, r.unit, r.item, r.prep_note)
    }

    fn line(q: &str, unit: &str, item: &str, note: &str) -> (String, String, String, String) {
        (q.into(), unit.into(), item.into(), note.into())
    }

    fn heading(line: &str) -> String {
        let r = parse_line(line).unwrap();
        assert!(r.is_heading, "{line}");
        r.item
    }

    #[test]
    fn parses_the_brief_examples() {
        assert_eq!(
            p("1 1/2 cups flour, sifted"),
            line("1½", "cups", "flour", "sifted")
        );
        assert_eq!(p("2 large eggs"), line("2", "", "large eggs", ""));
        assert_eq!(p("½ tsp salt"), line("½", "tsp", "salt", ""));
        assert_eq!(
            p("2-3 cloves garlic, minced"),
            line("2–3", "cloves", "garlic", "minced")
        );
        assert_eq!(
            p("Salt and pepper to taste"),
            line("", "", "Salt and pepper to taste", "")
        );
    }

    #[test]
    fn parses_quantity_forms() {
        assert_eq!(p("1½ cups milk").0, "1½");
        assert_eq!(p("1 ½ cups milk").0, "1½");
        assert_eq!(p("3/4 cup sugar").0, "¾");
        assert_eq!(p("0.5 l water").0, "½");
        assert_eq!(p("2 to 3 tbsp honey"), line("2–3", "tbsp", "honey", ""));
        assert_eq!(p("2 - 3 tbsp honey").0, "2–3");
        assert_eq!(p("1-1/2 cups oats").0, "1½");
        assert_eq!(p("250 ml stock"), line("250", "ml", "stock", ""));
    }

    #[test]
    fn parses_units() {
        assert_eq!(p("200g butter"), line("200", "g", "butter", ""));
        assert_eq!(p("1½cups rice"), line("1½", "cups", "rice", ""));
        assert_eq!(p("3 tbsp. olive oil"), line("3", "tbsp", "olive oil", ""));
        assert_eq!(p("1 lb. ground beef"), line("1", "lb", "ground beef", ""));
        assert_eq!(p("2 fl oz rum"), line("2", "fl oz", "rum", ""));
        assert_eq!(p("2 cups of milk"), line("2", "cups", "milk", ""));
        assert_eq!(p("1 Tbsp butter"), line("1", "Tbsp", "butter", ""));
        assert_eq!(
            p("1 (14 oz) can diced tomatoes, drained"),
            line("1", "(14 oz) can", "diced tomatoes", "drained")
        );
        assert_eq!(p("2 (optional) eggs"), line("2", "", "(optional) eggs", ""));
        assert_eq!(p("2 cloves"), line("2", "", "cloves", ""));
    }

    #[test]
    fn parses_units_without_quantity() {
        assert_eq!(p("Pinch of salt"), line("", "Pinch", "salt", ""));
        assert_eq!(p("a pinch of salt"), line("1", "pinch", "salt", ""));
        assert_eq!(
            p("a few sprigs thyme"),
            line("", "", "a few sprigs thyme", "")
        );
        assert_eq!(p("Can opener"), line("", "", "Can opener", ""));
    }

    #[test]
    fn parses_prep_notes() {
        assert_eq!(
            p("1 onion (finely chopped)"),
            line("1", "", "onion", "finely chopped")
        );
        assert_eq!(
            p("1 lb chicken thighs, boneless, skinless"),
            line("1", "lb", "chicken thighs", "boneless, skinless")
        );
        assert_eq!(
            p("(optional) chives"),
            line("", "", "(optional) chives", "")
        );
    }

    #[test]
    fn leaves_odd_lines_in_the_item() {
        assert_eq!(p("Juice of 1 lemon"), line("", "", "Juice of 1 lemon", ""));
        assert_eq!(
            p("1 15-ounce can beans"),
            line("1", "", "15-ounce can beans", "")
        );
        assert_eq!(p("3 4-inch pieces ginger").0, "3");
        assert_eq!(p("V8 juice"), line("", "", "V8 juice", ""));
    }

    #[test]
    fn strips_list_markers() {
        assert_eq!(p("• 3 eggs"), line("3", "", "eggs", ""));
        assert_eq!(p("- 1 cup flour"), line("1", "cup", "flour", ""));
        assert_eq!(p("▢ 2 tsp vanilla"), line("2", "tsp", "vanilla", ""));
        assert_eq!(p("*  1   cup   sugar "), line("1", "cup", "sugar", ""));
        assert_eq!(parse_line("  •  "), None);
    }

    #[test]
    fn detects_headings() {
        assert_eq!(heading("For the sauce:"), "For the sauce");
        assert_eq!(heading("For the topping"), "For the topping");
        assert_eq!(heading("SAUCE"), "SAUCE");
        assert_eq!(heading("## Dough"), "Dough");
        assert_eq!(heading("Frosting :"), "Frosting");
        assert!(!parse_line("For frying, vegetable oil").unwrap().is_heading);
        assert!(!parse_line("2 EGGS").unwrap().is_heading);
    }

    #[test]
    fn parses_a_block() {
        let text = "Dough:\n\n2 cups flour\n1 tsp salt\n\nFilling:\n3 apples, sliced\n";
        let lines = parse_block(text);
        let summary: Vec<(bool, &str)> = lines
            .iter()
            .map(|l| (l.is_heading, l.item.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                (true, "Dough"),
                (false, "flour"),
                (false, "salt"),
                (true, "Filling"),
                (false, "apples")
            ]
        );
    }

    #[test]
    fn quantities_round_trip_through_the_editor() {
        for l in [
            "1 1/2 cups flour",
            "2-3 eggs",
            "0.15 g saffron",
            "⅓ cup oil",
        ] {
            let q = p(l).0;
            assert!(quantity::parse(&q).unwrap().is_some(), "{q}");
        }
    }
}
