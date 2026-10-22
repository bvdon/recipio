// Typed wrappers around the Rust commands. All business logic lives in Rust.
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { Label } from "./bindings/Label";
import type { LabelKind } from "./bindings/LabelKind";
import type { Recipe } from "./bindings/Recipe";
import type { RecipeInput } from "./bindings/RecipeInput";
import type { RecipeQuery } from "./bindings/RecipeQuery";
import type { RecipeSummary } from "./bindings/RecipeSummary";

export type AppError = { kind: string; message: string };

export function errorMessage(err: unknown): string {
  if (err && typeof err === "object" && "message" in err) {
    return String((err as AppError).message);
  }
  return String(err);
}

export const api = {
  newRecipeId: () => invoke<string>("new_recipe_id"),
  listRecipes: (query: RecipeQuery) => invoke<RecipeSummary[]>("list_recipes", { query }),
  getRecipe: (id: string) => invoke<Recipe>("get_recipe", { id }),
  saveRecipe: (input: RecipeInput) => invoke<Recipe>("save_recipe", { input }),
  deleteRecipe: (id: string) => invoke<void>("delete_recipe", { id }),
  stagePhoto: (path: string) => invoke<string>("stage_photo", { path }),
  checkQuantity: (text: string) => invoke<boolean>("check_quantity", { text }),
  listLabels: (kind: LabelKind) => invoke<Label[]>("list_labels", { kind }),
  createLabel: (kind: LabelKind, name: string) => invoke<number>("create_label", { kind, name }),
  renameLabel: (kind: LabelKind, id: number, name: string) =>
    invoke<void>("rename_label", { kind, id, name }),
  deleteLabel: (kind: LabelKind, id: number) => invoke<void>("delete_label", { kind, id }),
  mergeLabels: (kind: LabelKind, sources: number[], target: number) =>
    invoke<void>("merge_labels", { kind, sources, target }),
};

/** URL for a file in the media folder. `version` busts the webview cache after a replace. */
export function mediaUrl(relPath: string, version?: string): string {
  const url = convertFileSrc(relPath, "recipio-media");
  return version ? `${url}?v=${encodeURIComponent(version)}` : url;
}

export const queryKeys = {
  recipes: ["recipes"] as const,
  recipeList: (query: RecipeQuery) => ["recipes", query] as const,
  recipe: (id: string) => ["recipe", id] as const,
  /** Prefix of every `recipe(id)` key. */
  allRecipes: ["recipe"] as const,
  labels: (kind: LabelKind) => ["labels", kind] as const,
  allLabels: ["labels"] as const,
};
