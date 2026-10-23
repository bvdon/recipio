import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useBlocker, useLocation, useNavigate, useParams } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { Globe, ImagePlus, Save, Trash2 } from "lucide-react";
import { api, errorMessage, mediaUrl, queryKeys } from "../api";
import type { Recipe } from "../bindings/Recipe";
import type { RecipeInput } from "../bindings/RecipeInput";
import type { PhotoChange } from "../bindings/PhotoChange";
import type { VideoInfo } from "../bindings/VideoInfo";
import type { WebImport } from "../bindings/WebImport";
import { ErrorBanner, PhotoPlaceholder, StarPicker } from "../components/common";
import LabelPicker from "../components/LabelPicker";
import IngredientsEditor, { blankRow, type IngredientRow } from "../components/IngredientsEditor";
import StepsEditor, { blankStep, type StepRow } from "../components/StepsEditor";
import { VideoField } from "../components/VideoDownload";
import type { ImportState } from "../components/ImportRecipeDialog";

type Form = {
  title: string;
  description: string;
  servings: string;
  prep: string;
  cook: string;
  notes: string;
  rating: number | null;
  youtube_url: string;
  source_url: string;
  calories: string;
  protein: string;
  carbs: string;
  fat: string;
  ingredients: IngredientRow[];
  steps: StepRow[];
  categories: string[];
  tags: string[];
  photo: PhotoChange;
};

const str = (v: number | null) => (v === null ? "" : String(v));

function toForm(r: Recipe | null): Form {
  return {
    title: r?.title ?? "",
    description: r?.description ?? "",
    servings: str(r?.servings ?? null),
    prep: str(r?.prep_minutes ?? null),
    cook: str(r?.cook_minutes ?? null),
    notes: r?.notes ?? "",
    rating: r?.rating ?? null,
    youtube_url: r?.youtube_url ?? "",
    source_url: r?.source_url ?? "",
    calories: str(r?.nutrition.calories ?? null),
    protein: str(r?.nutrition.protein_g ?? null),
    carbs: str(r?.nutrition.carbs_g ?? null),
    fat: str(r?.nutrition.fat_g ?? null),
    ingredients: r?.ingredients.length
      ? r.ingredients.map((l) => ({
          key: crypto.randomUUID(),
          is_heading: l.is_heading,
          qty: l.quantity_text,
          unit: l.unit ?? "",
          item: l.item,
          prep_note: l.prep_note ?? "",
        }))
      : [blankRow()],
    steps: r?.steps.length ? r.steps.map((s) => blankStep(s)) : [blankStep()],
    categories: r?.categories ?? [],
    tags: r?.tags ?? [],
    photo: { action: "keep" },
  };
}

/** A blank form filled with what a web import found. */
function fromImport(w: WebImport): Form {
  const blank = toForm(null);
  if (w.outcome === "no_recipe") return { ...blank, source_url: w.source_url };
  const r = w.recipe;
  return {
    ...blank,
    title: r.title,
    description: r.description,
    servings: str(r.servings),
    prep: str(r.prep_minutes),
    cook: str(r.cook_minutes),
    youtube_url: r.youtube_url ?? "",
    source_url: r.source_url,
    calories: str(r.nutrition.calories),
    protein: str(r.nutrition.protein_g),
    carbs: str(r.nutrition.carbs_g),
    fat: str(r.nutrition.fat_g),
    ingredients: r.ingredients.length
      ? r.ingredients.map((l) => ({
          key: crypto.randomUUID(),
          is_heading: l.is_heading,
          qty: l.quantity,
          unit: l.unit,
          item: l.item,
          prep_note: l.prep_note,
        }))
      : blank.ingredients,
    steps: r.steps.length ? r.steps.map((s) => blankStep(s)) : blank.steps,
    categories: r.categories,
    tags: r.tags,
    photo: r.photo ? { action: "replace", staged_path: r.photo } : blank.photo,
  };
}

/** Converts the form to the save payload, or returns a list of problems. */
function toInput(id: string, f: Form): RecipeInput | string[] {
  const problems: string[] = [];
  const int = (label: string, v: string, min: number): number | null => {
    if (v.trim() === "") return null;
    const n = Number(v);
    if (!Number.isInteger(n) || n < min) {
      problems.push(`${label} must be a whole number${min > 0 ? " of at least 1" : ""}.`);
      return null;
    }
    return n;
  };
  const num = (label: string, v: string): number | null => {
    if (v.trim() === "") return null;
    const n = Number(v.replace(",", "."));
    if (!Number.isFinite(n) || n < 0) {
      problems.push(`${label} must be a positive number.`);
      return null;
    }
    return n;
  };

  if (f.title.trim() === "") problems.push("Please give the recipe a title.");
  // Quantities are sent as typed; Rust parses and reports anything it can't read.
  const ingredients = f.ingredients.map((row) => ({
    is_heading: row.is_heading,
    quantity: row.is_heading ? "" : row.qty,
    unit: row.unit || null,
    item: row.item,
    prep_note: row.prep_note || null,
  }));

  const input: RecipeInput = {
    id,
    title: f.title,
    description: f.description,
    servings: int("Servings", f.servings, 1),
    prep_minutes: int("Prep time", f.prep, 0),
    cook_minutes: int("Cook time", f.cook, 0),
    photo: f.photo,
    notes: f.notes,
    rating: f.rating,
    youtube_url: f.youtube_url || null,
    source_url: f.source_url || null,
    nutrition: {
      calories: num("Calories", f.calories),
      protein_g: num("Protein", f.protein),
      carbs_g: num("Carbs", f.carbs),
      fat_g: num("Fat", f.fat),
    },
    ingredients,
    steps: f.steps.map((s) => s.text),
    categories: f.categories,
    tags: f.tags,
  };
  return problems.length ? problems : input;
}

export default function RecipeEditor() {
  const { id: routeId } = useParams();
  const location = useLocation();
  const isNew = routeId === undefined;
  const webImport = isNew ? ((location.state as ImportState | null)?.webImport ?? null) : null;
  // A new recipe gets its id up front so media (e.g. a video download) can be named after it.
  // Keyed by the history entry, so opening an import from a blank editor starts afresh.
  const newId = useQuery({
    queryKey: ["new-recipe-id", location.key],
    queryFn: api.newRecipeId,
    enabled: isNew,
    gcTime: 0,
    staleTime: Infinity,
  });
  const existing = useQuery({
    queryKey: queryKeys.recipe(routeId ?? ""),
    queryFn: () => api.getRecipe(routeId!),
    enabled: !isNew,
  });

  const id = isNew ? newId.data : routeId;
  const error = newId.error ?? existing.error;
  if (error) return <ErrorBanner message={errorMessage(error)} />;
  if (!id || (!isNew && !existing.data)) return null;
  return (
    <EditorForm key={id} id={id} recipe={isNew ? null : existing.data!} webImport={webImport} />
  );
}

function EditorForm({
  id,
  recipe,
  webImport,
}: {
  id: string;
  recipe: Recipe | null;
  /** Pre-fills a new recipe; the form then counts as changed until saved. */
  webImport: WebImport | null;
}) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const initial = useMemo(() => toForm(recipe), [recipe]);
  const [form, setForm] = useState<Form>(() => (webImport ? fromImport(webImport) : initial));
  const [problems, setProblems] = useState<string[]>([]);
  const saved = useRef(false);
  const savedPhoto = recipe?.photo_path ? mediaUrl(recipe.photo_path, recipe.updated_at) : null;
  const photoPreview =
    form.photo.action === "replace"
      ? mediaUrl(form.photo.staged_path)
      : form.photo.action === "remove"
        ? null
        : savedPhoto;

  const set = <K extends keyof Form>(key: K, value: Form[K]) =>
    setForm((f) => ({ ...f, [key]: value }));

  const dirty = useMemo(() => JSON.stringify(form) !== JSON.stringify(initial), [form, initial]);

  const save = useMutation({
    mutationFn: api.saveRecipe,
    onSuccess: (r) => {
      saved.current = true;
      queryClient.setQueryData(queryKeys.recipe(r.id), r);
      void queryClient.invalidateQueries({ queryKey: queryKeys.recipes });
      void queryClient.invalidateQueries({ queryKey: queryKeys.allLabels });
      navigate(`/recipes/${r.id}`, { replace: recipe === null });
    },
  });

  const submit = useCallback(() => {
    const result = toInput(id, form);
    if (Array.isArray(result)) {
      setProblems(result);
      return;
    }
    setProblems([]);
    save.mutate(result);
  }, [id, form, save]);

  // Cmd/Ctrl+S saves.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        submit();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [submit]);

  // Confirm before leaving with unsaved changes.
  const blocker = useBlocker(() => dirty && !saved.current);
  useEffect(() => {
    if (blocker.state !== "blocked") return;
    void ask("You have unsaved changes. Leave without saving?", {
      title: "Unsaved changes",
      kind: "warning",
      okLabel: "Discard changes",
      cancelLabel: "Keep editing",
    }).then((leave) => {
      if (!leave) return blocker.reset();
      // A never-saved recipe's download has nothing to attach to.
      if (recipe === null) void api.removeVideo(id);
      blocker.proceed();
    });
  }, [blocker, recipe, id]);

  const choosePhoto = async () => {
    const path = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "Images", extensions: ["jpg", "jpeg", "png", "webp", "gif", "avif"] }],
    });
    if (!path) return;
    try {
      const staged = await api.stagePhoto(path);
      set("photo", { action: "replace", staged_path: staged });
    } catch (e) {
      setProblems([errorMessage(e)]);
    }
  };

  const removePhoto = () => set("photo", { action: "remove" });

  // A pasted YouTube link fills in the title and photo, but never replaces what's there.
  const applyVideoInfo = useCallback(
    (info: VideoInfo) =>
      setForm((f) => {
        const hasPhoto =
          f.photo.action === "replace" || (f.photo.action === "keep" && savedPhoto !== null);
        return {
          ...f,
          title: f.title.trim() ? f.title : info.title,
          photo:
            !hasPhoto && info.thumbnail
              ? { action: "replace", staged_path: info.thumbnail }
              : f.photo,
        };
      }),
    [savedPhoto],
  );

  return (
    <form
      className="editor"
      noValidate
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <div className="toolbar">
        <h1>{recipe ? "Edit recipe" : "New recipe"}</h1>
        <div className="actions">
          <Link to={recipe ? `/recipes/${recipe.id}` : "/"} className="button ghost">
            Cancel
          </Link>
          <button
            type="submit"
            className="button primary"
            disabled={save.isPending}
            title="Save (⌘/Ctrl+S)"
          >
            <Save size={16} aria-hidden /> Save
          </button>
        </div>
      </div>

      {webImport && <ImportNotice webImport={webImport} />}
      {problems.length > 0 && <ErrorBanner message={problems.join(" ")} />}
      {save.isError && <ErrorBanner message={errorMessage(save.error)} />}

      <div className="editor-top">
        <div className="photo-field">
          {photoPreview ? <img src={photoPreview} alt="Recipe photo" /> : <PhotoPlaceholder />}
          <div className="row-actions">
            <button type="button" className="button" onClick={() => void choosePhoto()}>
              <ImagePlus size={16} aria-hidden /> {photoPreview ? "Replace" : "Add photo"}
            </button>
            {photoPreview && (
              <button type="button" className="button ghost" onClick={removePhoto}>
                <Trash2 size={16} aria-hidden /> Remove
              </button>
            )}
          </div>
        </div>

        <div className="fields">
          <label>
            Title
            <input
              value={form.title}
              onChange={(e) => set("title", e.target.value)}
              autoFocus={recipe === null}
              required
            />
          </label>
          <label>
            Description
            <textarea
              value={form.description}
              onChange={(e) => set("description", e.target.value)}
              rows={3}
            />
          </label>
          <div className="field-row">
            <label>
              Servings
              <input
                value={form.servings}
                onChange={(e) => set("servings", e.target.value)}
                inputMode="numeric"
              />
            </label>
            <label>
              Prep (min)
              <input
                value={form.prep}
                onChange={(e) => set("prep", e.target.value)}
                inputMode="numeric"
              />
            </label>
            <label>
              Cook (min)
              <input
                value={form.cook}
                onChange={(e) => set("cook", e.target.value)}
                inputMode="numeric"
              />
            </label>
            <div className="label">
              Rating
              <StarPicker value={form.rating} onChange={(v) => set("rating", v)} />
            </div>
          </div>
          <LabelPicker
            kind="category"
            label="Categories"
            value={form.categories}
            onChange={(v) => set("categories", v)}
          />
          <LabelPicker kind="tag" label="Tags" value={form.tags} onChange={(v) => set("tags", v)} />
        </div>
      </div>

      <section>
        <h2>Ingredients</h2>
        <IngredientsEditor rows={form.ingredients} onChange={(rows) => set("ingredients", rows)} />
      </section>

      <section>
        <h2>Steps</h2>
        <StepsEditor steps={form.steps} onChange={(steps) => set("steps", steps)} />
      </section>

      <section>
        <h2>Notes</h2>
        <textarea
          value={form.notes}
          onChange={(e) => set("notes", e.target.value)}
          rows={4}
          aria-label="Notes"
        />
      </section>

      <section>
        <h2>Video</h2>
        <VideoField
          recipeId={id}
          value={form.youtube_url}
          onChange={(url) => set("youtube_url", url)}
          saved={{ status: recipe?.video_status ?? "none", url: recipe?.youtube_url ?? null }}
          onInfo={applyVideoInfo}
        />
      </section>

      <section>
        <h2>Source</h2>
        <label>
          Original web page
          <input
            value={form.source_url}
            onChange={(e) => set("source_url", e.target.value)}
            placeholder="https://…"
            type="url"
          />
        </label>
      </section>

      <details className="nutrition-editor">
        <summary>Nutrition per serving (optional)</summary>
        <div className="field-row">
          <label>
            Calories
            <input
              value={form.calories}
              onChange={(e) => set("calories", e.target.value)}
              inputMode="decimal"
            />
          </label>
          <label>
            Protein (g)
            <input
              value={form.protein}
              onChange={(e) => set("protein", e.target.value)}
              inputMode="decimal"
            />
          </label>
          <label>
            Carbs (g)
            <input
              value={form.carbs}
              onChange={(e) => set("carbs", e.target.value)}
              inputMode="decimal"
            />
          </label>
          <label>
            Fat (g)
            <input
              value={form.fat}
              onChange={(e) => set("fat", e.target.value)}
              inputMode="decimal"
            />
          </label>
        </div>
      </details>
    </form>
  );
}

function ImportNotice({ webImport }: { webImport: WebImport }) {
  const url = webImport.outcome === "found" ? webImport.recipe.source_url : webImport.source_url;
  let host = url;
  try {
    host = new URL(url).hostname.replace(/^www\./, "");
  } catch {
    // Show the address as is.
  }
  const warnings = webImport.outcome === "found" ? webImport.recipe.warnings : [];
  return (
    <div className="import-notice" role="status">
      <Globe size={18} aria-hidden />
      <div>
        {webImport.outcome === "found"
          ? `Imported from ${host}. Check everything, then save.`
          : `${host} had no recipe information, so only the link was kept.`}
        {warnings.length > 0 && (
          <ul>
            {warnings.map((w) => (
              <li key={w}>{w}</li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
