import { useMemo, useState } from "react";
import { Link, useSearchParams } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, Search, Send, Video } from "lucide-react";
import { api, errorMessage, mediaUrl, queryKeys } from "../api";
import type { RecipeQuery } from "../bindings/RecipeQuery";
import type { RecipeSummary } from "../bindings/RecipeSummary";
import { ErrorBanner, PhotoPlaceholder, plural } from "../components/common";
import ShareDialog from "../components/ShareDialog";

const ALL: RecipeQuery = {
  search: "",
  category_ids: [],
  tag_ids: [],
  sort: "title",
  descending: false,
};

/**
 * Pick recipes to send to a friend (or save). `?all=1` starts with every recipe ticked
 * ("share the whole book"); `?friend=<id>` preselects who to send to.
 */
export default function ShareRecipes() {
  const recipes = useQuery({
    queryKey: queryKeys.recipeList(ALL),
    queryFn: () => api.listRecipes(ALL),
  });
  const [params] = useSearchParams();
  const friend = Number(params.get("friend")) || null;

  return (
    <div className="share-page">
      <div className="toolbar">
        <Link to="/friends" className="button ghost">
          <ArrowLeft size={16} aria-hidden /> Friends
        </Link>
      </div>
      <h1>Share recipes</h1>
      <p className="muted">
        Choose recipes, then send them to a friend — as a Recipio file they can add to their
        library, or as a PDF anyone can read. Photos, notes and video bookmarks go along; your star
        ratings don’t.
      </p>
      {recipes.isError && <ErrorBanner message={errorMessage(recipes.error)} />}
      {recipes.data && (
        <Picker recipes={recipes.data} all={params.get("all") === "1"} friend={friend} />
      )}
    </div>
  );
}

function Picker({
  recipes,
  all,
  friend,
}: {
  recipes: RecipeSummary[];
  all: boolean;
  friend: number | null;
}) {
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState<ReadonlySet<string>>(
    () => new Set(all ? recipes.map((r) => r.id) : []),
  );
  const [sharing, setSharing] = useState(false);

  const shown = useMemo(() => {
    const words = filter.toLowerCase().split(/\s+/).filter(Boolean);
    return recipes.filter((r) => words.every((w) => r.title.toLowerCase().includes(w)));
  }, [recipes, filter]);

  const toggle = (id: string) =>
    setSelected((s) => {
      const next = new Set(s);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  const allShown = shown.length > 0 && shown.every((r) => selected.has(r.id));
  const toggleShown = () =>
    setSelected((s) => {
      const next = new Set(s);
      for (const r of shown) {
        if (allShown) next.delete(r.id);
        else next.add(r.id);
      }
      return next;
    });

  return (
    <>
      <div className="share-controls">
        <div className="search">
          <Search size={16} aria-hidden />
          <input
            type="search"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Find recipes"
            aria-label="Find recipes"
          />
        </div>
        <button
          type="button"
          className="button small"
          onClick={toggleShown}
          disabled={shown.length === 0}
        >
          {allShown ? "Clear" : filter ? "Select these" : "Select all"}
        </button>
        <span className="muted">{plural(selected.size, "recipe")} selected</span>
      </div>

      <ul className="pick-list">
        {shown.map((r) => (
          <li key={r.id} className={selected.has(r.id) ? "on" : undefined}>
            <label>
              <input type="checkbox" checked={selected.has(r.id)} onChange={() => toggle(r.id)} />
              {r.photo_path ? (
                <img src={mediaUrl(r.photo_path, r.updated_at)} alt="" />
              ) : (
                <PhotoPlaceholder />
              )}
              <span className="pick-title">{r.title}</span>
              {r.has_video && <Video size={16} aria-label="Has a video" className="muted" />}
            </label>
          </li>
        ))}
        {shown.length === 0 && (
          <li className="muted empty">
            {recipes.length ? "No recipes match." : "No recipes yet."}
          </li>
        )}
      </ul>

      <div className="share-footer">
        <span className="muted">
          {selected.size
            ? plural(selected.size, "recipe") + " selected"
            : "Select the recipes to share."}
        </span>
        <button
          type="button"
          className="button primary"
          disabled={selected.size === 0}
          onClick={() => setSharing(true)}
        >
          <Send size={16} aria-hidden /> Share…
        </button>
      </div>
      {sharing && (
        <ShareDialog ids={[...selected]} friendId={friend} onClose={() => setSharing(false)} />
      )}
    </>
  );
}
