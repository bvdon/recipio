// Typed wrappers around the Rust commands. All business logic lives in Rust.
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { Label } from "./bindings/Label";
import type { LabelKind } from "./bindings/LabelKind";
import type { ParsedIngredient } from "./bindings/ParsedIngredient";
import type { Recipe } from "./bindings/Recipe";
import type { RecipeInput } from "./bindings/RecipeInput";
import type { RecipeQuery } from "./bindings/RecipeQuery";
import type { RecipeSummary } from "./bindings/RecipeSummary";
import type { Package } from "./bindings/Package";
import type { ToolInfo } from "./bindings/ToolInfo";
import type { VideoJob } from "./bindings/VideoJob";
import type { WebImport } from "./bindings/WebImport";
import type { BackupSummary } from "./bindings/BackupSummary";
import type { LibraryStats } from "./bindings/LibraryStats";
import type { SharePreview } from "./bindings/SharePreview";
import type { VideoBookmark } from "./bindings/VideoBookmark";

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
  parseIngredients: (text: string) => invoke<ParsedIngredient[]>("parse_ingredients", { text }),
  listLabels: (kind: LabelKind) => invoke<Label[]>("list_labels", { kind }),
  createLabel: (kind: LabelKind, name: string) => invoke<number>("create_label", { kind, name }),
  renameLabel: (kind: LabelKind, id: number, name: string) =>
    invoke<void>("rename_label", { kind, id, name }),
  deleteLabel: (kind: LabelKind, id: number) => invoke<void>("delete_label", { kind, id }),
  mergeLabels: (kind: LabelKind, sources: number[], target: number) =>
    invoke<void>("merge_labels", { kind, sources, target }),
  checkYoutubeUrl: (url: string) => invoke<boolean>("check_youtube_url", { url }),
  startVideoDownload: (recipeId: string, url: string) =>
    invoke<VideoJob>("start_video_download", { recipeId, url }),
  cancelVideoDownload: (recipeId: string) => invoke<void>("cancel_video_download", { recipeId }),
  getVideoJob: (recipeId: string) => invoke<VideoJob | null>("get_video_job", { recipeId }),
  dismissVideoJob: (recipeId: string) => invoke<void>("dismiss_video_job", { recipeId }),
  removeVideo: (recipeId: string) => invoke<void>("remove_video", { recipeId }),
  toolsStatus: () => invoke<ToolInfo[]>("tools_status"),
  installTools: (update: boolean) => invoke<Package[]>("install_tools", { update }),
  importRecipe: (url: string) => invoke<WebImport>("import_recipe", { url }),
  setKeepAwake: (on: boolean) => invoke<void>("set_keep_awake", { on }),
  libraryStats: (ids?: string[]) => invoke<LibraryStats>("library_stats", { ids: ids ?? null }),
  exportBackup: (path: string, includeVideos: boolean) =>
    invoke<void>("export_backup", { path, includeVideos }),
  cancelBackup: () => invoke<void>("cancel_backup"),
  inspectBackup: (path: string) => invoke<BackupSummary>("inspect_backup", { path }),
  restoreBackup: (path: string) => invoke<number>("restore_backup", { path }),
  shareRecipes: (path: string, ids: string[], includeVideos: boolean) =>
    invoke<void>("share_recipes", { path, ids, includeVideos }),
  previewShared: (path: string) => invoke<SharePreview>("preview_shared", { path }),
  addSharedRecipes: (path: string, ids: string[]) =>
    invoke<number>("add_shared_recipes", { path, ids }),
  addVideoBookmark: (recipeId: string, time: string, label: string, duration: number | null) =>
    invoke<VideoBookmark[]>("add_video_bookmark", { recipeId, time, label, duration }),
  exportPdf: (id: string, path: string, locale: string) =>
    invoke<void>("export_pdf", { id, path, locale }),
  deleteVideoBookmark: (recipeId: string, id: number) =>
    invoke<VideoBookmark[]>("delete_video_bookmark", { recipeId, id }),
};

/** Tauri event names (see `video.rs` and `commands::install_tools`). */
export const events = {
  videoJob: "video://job",
  toolsProgress: "tools://progress",
  backupProgress: "backup://progress",
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
  videoJob: (recipeId: string) => ["video-job", recipeId] as const,
  tools: ["tools"] as const,
  libraryStats: ["library-stats"] as const,
};
