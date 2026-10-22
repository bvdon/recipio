// Typed wrappers around the Rust commands. All business logic lives in Rust.
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { Recipe } from "./bindings/Recipe";
import type { RecipeInput } from "./bindings/RecipeInput";
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
  listRecipes: () => invoke<RecipeSummary[]>("list_recipes"),
  getRecipe: (id: string) => invoke<Recipe>("get_recipe", { id }),
  saveRecipe: (input: RecipeInput) => invoke<Recipe>("save_recipe", { input }),
  deleteRecipe: (id: string) => invoke<void>("delete_recipe", { id }),
  stagePhoto: (path: string) => invoke<string>("stage_photo", { path }),
};

/** URL for a file in the media folder. `version` busts the webview cache after a replace. */
export function mediaUrl(relPath: string, version?: string): string {
  const url = convertFileSrc(relPath, "recipio-media");
  return version ? `${url}?v=${encodeURIComponent(version)}` : url;
}

export const queryKeys = {
  recipes: ["recipes"] as const,
  recipe: (id: string) => ["recipe", id] as const,
};
