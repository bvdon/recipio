//! Scaling a recipe's ingredient amounts for the recipe page and its PDF. Nothing is saved:
//! the stored amounts stay as entered and scaling is applied to a copy on the way out.

use serde::Deserialize;
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::models::Recipe;
use crate::quantity;

/// Most servings (or the largest factor) the page offers.
const MAX_SERVINGS: u32 = 99;
const MAX_FACTOR: f64 = 10.0;

/// How to scale: to a number of servings (recipes that have servings), or by a factor
/// (recipes without).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, TS)]
#[serde(tag = "by", rename_all = "snake_case")]
#[ts(export)]
pub enum Scale {
    Servings { servings: u32 },
    Factor { factor: f64 },
}

/// Scales `recipe`'s ingredient amounts (and servings) in place. Lines without an amount
/// ("salt to taste") and units are left as they are; amounts are rounded to kitchen-friendly
/// fractions.
pub fn apply(recipe: &mut Recipe, scale: Scale) -> AppResult<()> {
    let factor = match scale {
        Scale::Servings { servings } => {
            let Some(from) = recipe.servings else {
                return Err(AppError::Validation(
                    "This recipe has no servings to scale from.".into(),
                ));
            };
            if !(1..=MAX_SERVINGS).contains(&servings) {
                return Err(AppError::Validation(format!(
                    "Servings must be between 1 and {MAX_SERVINGS}."
                )));
            }
            recipe.servings = Some(servings);
            f64::from(servings) / f64::from(from)
        }
        Scale::Factor { factor } => {
            if !(factor.is_finite() && factor > 0.0 && factor <= MAX_FACTOR) {
                return Err(AppError::Validation(format!(
                    "The scale must be more than 0 and at most {MAX_FACTOR}."
                )));
            }
            factor
        }
    };
    if (factor - 1.0).abs() < f64::EPSILON {
        return Ok(());
    }
    for line in recipe.ingredients.iter_mut().filter(|l| !l.is_heading) {
        let Some(min) = line.quantity else { continue };
        let min = friendly(min * factor);
        let max = line
            .quantity_max
            .map(|m| friendly(m * factor))
            .filter(|m| *m > min);
        line.quantity = Some(min);
        line.quantity_max = max;
        line.quantity_text = match max {
            None => amount_text(min),
            Some(max) => format!("{}–{}", amount_text(min), amount_text(max)),
        };
    }
    Ok(())
}

/// Rounds to what a cook can measure: eighths and thirds below 1, quarters and thirds up
/// to 10, whole numbers above. Never rounds a real amount down to nothing (1/16 minimum).
fn friendly(q: f64) -> f64 {
    if q >= 10.0 {
        return q.round();
    }
    if q < 3.0 / 32.0 {
        return 1.0 / 16.0;
    }
    let steps: &[f64] = if q < 1.0 { &[8.0, 3.0] } else { &[4.0, 3.0] };
    steps
        .iter()
        .map(|d| (q * d).round() / d)
        .min_by(|a, b| (a - q).abs().total_cmp(&(b - q).abs()))
        .expect("steps is not empty")
}

fn amount_text(q: f64) -> String {
    if (q - 1.0 / 16.0).abs() < 1e-9 {
        "1/16".into()
    } else {
        quantity::format(Some(q), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{IngredientLine, Nutrition, VideoStatus};

    fn line(
        quantity: Option<f64>,
        quantity_max: Option<f64>,
        unit: &str,
        item: &str,
    ) -> IngredientLine {
        IngredientLine {
            is_heading: false,
            quantity,
            quantity_max,
            unit: Some(unit.into()).filter(|u: &String| !u.is_empty()),
            item: item.into(),
            prep_note: None,
            quantity_text: quantity::format(quantity, quantity_max),
        }
    }

    fn recipe(servings: Option<u32>) -> Recipe {
        Recipe {
            id: "r".into(),
            title: "Test".into(),
            description: String::new(),
            servings,
            prep_minutes: None,
            cook_minutes: None,
            photo_path: None,
            notes: String::new(),
            rating: None,
            youtube_url: None,
            video_path: None,
            video_status: VideoStatus::None,
            source_url: None,
            nutrition: Nutrition::default(),
            ingredients: vec![
                IngredientLine {
                    is_heading: true,
                    quantity: None,
                    quantity_max: None,
                    unit: None,
                    item: "Dough".into(),
                    prep_note: None,
                    quantity_text: String::new(),
                },
                line(Some(2.0), None, "cups", "flour"),
                line(Some(1.0 / 3.0), None, "cup", "sugar"),
                line(Some(2.0), Some(3.0), "", "eggs"),
                line(Some(0.125), None, "tsp", "nutmeg"),
                line(None, None, "", "salt to taste"),
                line(Some(250.0), None, "g", "butter"),
            ],
            steps: vec![],
            categories: vec![],
            tags: vec![],
            bookmarks: vec![],
            shared_by: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn texts(r: &Recipe) -> Vec<&str> {
        r.ingredients
            .iter()
            .map(|l| l.quantity_text.as_str())
            .collect()
    }

    #[test]
    fn scales_to_servings() {
        let mut r = recipe(Some(4));
        apply(&mut r, Scale::Servings { servings: 6 }).unwrap();
        assert_eq!(r.servings, Some(6));
        assert_eq!(texts(&r), ["", "3", "½", "3–4½", "¼", "", "375"]);
        assert_eq!(r.ingredients[0].item, "Dough");
        assert_eq!(r.ingredients[1].unit.as_deref(), Some("cups"));

        let mut r = recipe(Some(4));
        apply(&mut r, Scale::Servings { servings: 2 }).unwrap();
        assert_eq!(texts(&r), ["", "1", "⅛", "1–1½", "1/16", "", "125"]);
    }

    #[test]
    fn scales_by_factor_and_keeps_originals_at_one() {
        let mut r = recipe(None);
        apply(&mut r, Scale::Factor { factor: 3.0 }).unwrap();
        assert_eq!(r.servings, None);
        assert_eq!(texts(&r), ["", "6", "1", "6–9", "⅜", "", "750"]);

        let mut r = recipe(Some(4));
        apply(&mut r, Scale::Servings { servings: 4 }).unwrap();
        assert_eq!(texts(&r), texts(&recipe(Some(4))));
    }

    #[test]
    fn rounds_to_measurable_amounts() {
        assert_eq!(friendly(0.4), 3.0 / 8.0);
        assert_eq!(friendly(0.3), 1.0 / 3.0);
        assert_eq!(friendly(1.1), 1.0);
        assert_eq!(amount_text(friendly(2.6)), "2⅔");
        assert_eq!(friendly(12.4), 12.0);
        assert_eq!(friendly(0.01), 1.0 / 16.0);
    }

    #[test]
    fn refuses_bad_scales() {
        assert!(apply(&mut recipe(None), Scale::Servings { servings: 2 }).is_err());
        assert!(apply(&mut recipe(Some(4)), Scale::Servings { servings: 0 }).is_err());
        assert!(apply(&mut recipe(Some(4)), Scale::Servings { servings: 100 }).is_err());
        assert!(apply(&mut recipe(None), Scale::Factor { factor: 0.0 }).is_err());
        assert!(apply(&mut recipe(None), Scale::Factor { factor: f64::NAN }).is_err());
        assert!(apply(&mut recipe(None), Scale::Factor { factor: 11.0 }).is_err());
    }
}
