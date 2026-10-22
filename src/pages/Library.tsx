import { useState } from "react";
import { Link } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { LayoutGrid, List, Video } from "lucide-react";
import { api, errorMessage, mediaUrl, queryKeys } from "../api";
import type { RecipeSummary } from "../bindings/RecipeSummary";
import { ErrorBanner, PhotoPlaceholder, Stars } from "../components/common";

type ViewMode = "grid" | "list";
const VIEW_KEY = "recipio.libraryView";

function loadView(): ViewMode {
  try {
    return localStorage.getItem(VIEW_KEY) === "list" ? "list" : "grid";
  } catch {
    return "grid";
  }
}

export default function Library() {
  const [view, setView] = useState<ViewMode>(loadView);
  const recipes = useQuery({ queryKey: queryKeys.recipes, queryFn: api.listRecipes });

  const changeView = (v: ViewMode) => {
    setView(v);
    try {
      localStorage.setItem(VIEW_KEY, v);
    } catch {
      // Remembering the view is a convenience only.
    }
  };

  return (
    <section>
      <div className="toolbar">
        <h1>Recipes</h1>
        <div className="segmented" role="group" aria-label="Layout">
          <button
            className={view === "grid" ? "active" : ""}
            onClick={() => changeView("grid")}
            aria-pressed={view === "grid"}
            title="Grid"
          >
            <LayoutGrid size={16} aria-hidden />
          </button>
          <button
            className={view === "list" ? "active" : ""}
            onClick={() => changeView("list")}
            aria-pressed={view === "list"}
            title="List"
          >
            <List size={16} aria-hidden />
          </button>
        </div>
      </div>

      {recipes.isError && <ErrorBanner message={errorMessage(recipes.error)} />}
      {recipes.data?.length === 0 && (
        <div className="empty">
          <p>No recipes yet.</p>
          <Link to="/recipes/new" className="button primary">
            Add your first recipe
          </Link>
        </div>
      )}
      {recipes.data && recipes.data.length > 0 && (
        <ul className={view === "grid" ? "recipe-grid" : "recipe-list"}>
          {recipes.data.map((r) => (
            <RecipeCard key={r.id} recipe={r} />
          ))}
        </ul>
      )}
    </section>
  );
}

function RecipeCard({ recipe }: { recipe: RecipeSummary }) {
  return (
    <li>
      <Link to={`/recipes/${recipe.id}`} className="recipe-card">
        <div className="thumb">
          {recipe.photo_path ? (
            <img src={mediaUrl(recipe.photo_path, recipe.updated_at)} alt="" loading="lazy" />
          ) : (
            <PhotoPlaceholder />
          )}
          {recipe.has_video && (
            <span className="video-badge" title="Has video">
              <Video size={14} aria-label="Has video" />
            </span>
          )}
        </div>
        <div className="card-body">
          <h2>{recipe.title}</h2>
          {recipe.categories.length > 0 && (
            <div className="chips">
              {recipe.categories.map((c) => (
                <span key={c} className="chip">
                  {c}
                </span>
              ))}
            </div>
          )}
          {recipe.rating !== null && <Stars value={recipe.rating} />}
        </div>
      </Link>
    </li>
  );
}
