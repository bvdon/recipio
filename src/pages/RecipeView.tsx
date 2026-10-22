import { Link, useNavigate, useParams } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ask } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ArrowLeft, Clock, ExternalLink, Pencil, Trash2, Users } from "lucide-react";
import { api, errorMessage, mediaUrl, queryKeys } from "../api";
import type { Recipe } from "../bindings/Recipe";
import { ErrorBanner, formatMinutes, formatQuantity, Stars } from "../components/common";

export default function RecipeView() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const recipe = useQuery({ queryKey: queryKeys.recipe(id), queryFn: () => api.getRecipe(id) });

  const remove = useMutation({
    mutationFn: () => api.deleteRecipe(id),
    onSuccess: () => {
      // Leave the page first: removing the query while it is still observed triggers a
      // refetch of the now-deleted recipe.
      navigate("/");
      queryClient.removeQueries({ queryKey: queryKeys.recipe(id) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.recipes });
    },
  });

  const confirmDelete = async (r: Recipe) => {
    const hasFiles = r.photo_path !== null || r.video_path !== null;
    const ok = await ask(
      `Delete “${r.title}”?${hasFiles ? " Its photo and video files will also be deleted." : ""} This cannot be undone.`,
      { title: "Delete recipe", kind: "warning", okLabel: "Delete", cancelLabel: "Cancel" },
    );
    if (ok) remove.mutate();
  };

  if (recipe.isError) return <ErrorBanner message={errorMessage(recipe.error)} />;
  if (!recipe.data) return null;
  const r = recipe.data;
  const n = r.nutrition;
  const hasNutrition = [n.calories, n.protein_g, n.carbs_g, n.fat_g].some((v) => v !== null);

  return (
    <article className="recipe-view">
      <div className="toolbar">
        <Link to="/" className="button ghost">
          <ArrowLeft size={16} aria-hidden /> Recipes
        </Link>
        <div className="actions">
          <Link to={`/recipes/${r.id}/edit`} className="button">
            <Pencil size={16} aria-hidden /> Edit
          </Link>
          <button className="button danger" onClick={() => void confirmDelete(r)}>
            <Trash2 size={16} aria-hidden /> Delete
          </button>
        </div>
      </div>
      {remove.isError && <ErrorBanner message={errorMessage(remove.error)} />}

      <header className="recipe-header">
        {r.photo_path && <img className="hero" src={mediaUrl(r.photo_path, r.updated_at)} alt="" />}
        <div>
          <h1>{r.title}</h1>
          {r.rating !== null && <Stars value={r.rating} />}
          {r.description && <p className="description">{r.description}</p>}
          <ul className="meta">
            {r.servings !== null && (
              <li>
                <Users size={16} aria-hidden /> {r.servings} servings
              </li>
            )}
            {r.prep_minutes !== null && (
              <li>
                <Clock size={16} aria-hidden /> Prep {formatMinutes(r.prep_minutes)}
              </li>
            )}
            {r.cook_minutes !== null && (
              <li>
                <Clock size={16} aria-hidden /> Cook {formatMinutes(r.cook_minutes)}
              </li>
            )}
          </ul>
          {r.categories.length + r.tags.length > 0 && (
            <div className="chips">
              {r.categories.map((c) => (
                <span key={c} className="chip">
                  {c}
                </span>
              ))}
              {r.tags.map((t) => (
                <span key={t} className="chip tag">
                  #{t}
                </span>
              ))}
            </div>
          )}
        </div>
      </header>

      <div className="recipe-body">
        <section className="ingredients">
          <h2>Ingredients</h2>
          {r.ingredients.length === 0 && <p className="muted">No ingredients.</p>}
          <ul>
            {r.ingredients.map((l, i) =>
              l.is_heading ? (
                <li key={i} className="ingredient-heading">
                  {l.item}
                </li>
              ) : (
                <li key={i}>
                  <span className="qty">
                    {[formatQuantity(l.quantity, l.quantity_max), l.unit].filter(Boolean).join(" ")}
                  </span>{" "}
                  {l.item}
                  {l.prep_note && <span className="muted">, {l.prep_note}</span>}
                </li>
              ),
            )}
          </ul>
        </section>

        <section className="steps">
          <h2>Steps</h2>
          {r.steps.length === 0 && <p className="muted">No steps.</p>}
          <ol>
            {r.steps.map((s, i) => (
              <li key={i}>{s}</li>
            ))}
          </ol>
        </section>
      </div>

      {r.notes && (
        <section>
          <h2>Notes</h2>
          <p className="notes">{r.notes}</p>
        </section>
      )}

      {hasNutrition && (
        <section>
          <h2>Nutrition per serving</h2>
          <dl className="nutrition">
            {n.calories !== null && <Fact label="Calories" value={`${n.calories}`} />}
            {n.protein_g !== null && <Fact label="Protein" value={`${n.protein_g} g`} />}
            {n.carbs_g !== null && <Fact label="Carbs" value={`${n.carbs_g} g`} />}
            {n.fat_g !== null && <Fact label="Fat" value={`${n.fat_g} g`} />}
          </dl>
        </section>
      )}

      {(r.source_url || r.youtube_url) && (
        <section>
          <h2>Sources</h2>
          <ul className="links">
            {r.source_url && <SourceLink url={r.source_url} label="Original recipe" />}
            {r.youtube_url && <SourceLink url={r.youtube_url} label="YouTube" />}
          </ul>
        </section>
      )}
    </article>
  );
}

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

function SourceLink({ url, label }: { url: string; label: string }) {
  return (
    <li>
      <button className="link" onClick={() => void openUrl(url)} title={url}>
        {label} <ExternalLink size={14} aria-hidden />
      </button>
    </li>
  );
}
