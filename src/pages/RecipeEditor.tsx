import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useBlocker, useNavigate, useParams } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { ImagePlus, Save, Trash2 } from "lucide-react";
import { api, errorMessage, mediaUrl, queryKeys } from "../api";
import type { Recipe } from "../bindings/Recipe";
import type { RecipeInput } from "../bindings/RecipeInput";
import type { PhotoChange } from "../bindings/PhotoChange";
import { ErrorBanner, PhotoPlaceholder, StarPicker } from "../components/common";
import IngredientsEditor, {
  blankRow,
  type IngredientRow,
  parseQty,
} from "../components/IngredientsEditor";
import StepsEditor, { blankStep, type StepRow } from "../components/StepsEditor";

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
          qty:
            l.quantity === null
              ? ""
              : l.quantity_max === null
                ? String(l.quantity)
                : `${l.quantity}-${l.quantity_max}`,
          unit: l.unit ?? "",
          item: l.item,
          prep_note: l.prep_note ?? "",
        }))
      : [blankRow()],
    steps: r?.steps.length ? r.steps.map((s) => blankStep(s)) : [blankStep()],
    photo: { action: "keep" },
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
  const ingredients = f.ingredients.map((row) => {
    const q = row.is_heading ? null : parseQty(row.qty);
    if (q === "invalid")
      problems.push(`“${row.qty}” is not a valid quantity (use e.g. 1.5 or 2-3).`);
    const parsed = q === "invalid" ? null : q;
    return {
      is_heading: row.is_heading,
      quantity: parsed?.quantity ?? null,
      quantity_max: parsed?.quantity_max ?? null,
      unit: row.unit || null,
      item: row.item,
      prep_note: row.prep_note || null,
    };
  });

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
  };
  return problems.length ? problems : input;
}

export default function RecipeEditor() {
  const { id: routeId } = useParams();
  const isNew = routeId === undefined;
  // A new recipe gets its id up front so media (e.g. a video download) can be named after it.
  const newId = useQuery({
    queryKey: ["new-recipe-id"],
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
  return <EditorForm key={id} id={id} recipe={isNew ? null : existing.data!} />;
}

function EditorForm({ id, recipe }: { id: string; recipe: Recipe | null }) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const initial = useMemo(() => toForm(recipe), [recipe]);
  const [form, setForm] = useState<Form>(initial);
  const [problems, setProblems] = useState<string[]>([]);
  const [photoPreview, setPhotoPreview] = useState<string | null>(
    recipe?.photo_path ? mediaUrl(recipe.photo_path, recipe.updated_at) : null,
  );
  const saved = useRef(false);

  const set = <K extends keyof Form>(key: K, value: Form[K]) =>
    setForm((f) => ({ ...f, [key]: value }));

  const dirty = useMemo(() => JSON.stringify(form) !== JSON.stringify(initial), [form, initial]);

  const save = useMutation({
    mutationFn: api.saveRecipe,
    onSuccess: (r) => {
      saved.current = true;
      queryClient.setQueryData(queryKeys.recipe(r.id), r);
      void queryClient.invalidateQueries({ queryKey: queryKeys.recipes });
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
    }).then((leave) => (leave ? blocker.proceed() : blocker.reset()));
  }, [blocker]);

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
      setPhotoPreview(mediaUrl(staged));
    } catch (e) {
      setProblems([errorMessage(e)]);
    }
  };

  const removePhoto = () => {
    set("photo", { action: "remove" });
    setPhotoPreview(null);
  };

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
        <h2>Sources</h2>
        <div className="field-row">
          <label>
            YouTube URL
            <input
              value={form.youtube_url}
              onChange={(e) => set("youtube_url", e.target.value)}
              placeholder="https://www.youtube.com/watch?v=…"
              type="url"
            />
          </label>
          <label>
            Original web page
            <input
              value={form.source_url}
              onChange={(e) => set("source_url", e.target.value)}
              placeholder="https://…"
              type="url"
            />
          </label>
        </div>
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
