import { useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router";
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import {
  ArrowDownWideNarrow,
  ArrowUpNarrowWide,
  LayoutGrid,
  List,
  Search,
  FileDown,
  Settings2,
  Share2,
  Video,
  X,
} from "lucide-react";
import { api, errorMessage, mediaUrl, queryKeys } from "../api";
import type { Label } from "../bindings/Label";
import type { RecipeQuery } from "../bindings/RecipeQuery";
import type { RecipeSort } from "../bindings/RecipeSort";
import type { RecipeSummary } from "../bindings/RecipeSummary";
import { ErrorBanner, PhotoPlaceholder, Stars } from "../components/common";

type ViewMode = "grid" | "list";
type SortPref = { sort: RecipeSort; descending: boolean };

const VIEW_KEY = "recipio.libraryView";
const SORT_KEY = "recipio.librarySort";
const SORTS: { value: RecipeSort; label: string }[] = [
  { value: "title", label: "Title" },
  { value: "created", label: "Date added" },
  { value: "updated", label: "Date updated" },
  { value: "rating", label: "Rating" },
];
const DEFAULT_SORT: SortPref = { sort: "updated", descending: true };

function loadView(): ViewMode {
  try {
    return localStorage.getItem(VIEW_KEY) === "list" ? "list" : "grid";
  } catch {
    return "grid";
  }
}

function loadSort(): SortPref {
  try {
    const p = JSON.parse(localStorage.getItem(SORT_KEY) ?? "null") as SortPref | null;
    if (p && SORTS.some((s) => s.value === p.sort)) {
      return { sort: p.sort, descending: Boolean(p.descending) };
    }
  } catch {
    // Fall through to the default.
  }
  return DEFAULT_SORT;
}

function remember(key: string, value: unknown) {
  try {
    localStorage.setItem(key, typeof value === "string" ? value : JSON.stringify(value));
  } catch {
    // Remembering preferences is a convenience only.
  }
}

function useDebounced<T>(value: T, ms: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setDebounced(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return debounced;
}

const ids = (params: URLSearchParams, key: string) =>
  params
    .getAll(key)
    .map(Number)
    .filter((n) => Number.isInteger(n));

export default function Library() {
  const [view, setView] = useState<ViewMode>(loadView);
  const [sort, setSort] = useState<SortPref>(loadSort);
  // Search text and filters live in the URL so Back from a recipe restores them.
  const [params, setParams] = useSearchParams();
  const search = params.get("q") ?? "";
  const searchRef = useRef<HTMLInputElement>(null);

  const categories = useQuery({
    queryKey: queryKeys.labels("category"),
    queryFn: () => api.listLabels("category"),
  });
  const tags = useQuery({
    queryKey: queryKeys.labels("tag"),
    queryFn: () => api.listLabels("tag"),
  });

  // Drop ids of labels that no longer exist (deleted or merged since the URL was made).
  const known = (labels: Label[] | undefined, selected: number[]) =>
    labels ? selected.filter((id) => labels.some((l) => l.id === id)) : selected;
  const categoryIds = known(categories.data, ids(params, "c"));
  const tagIds = known(tags.data, ids(params, "t"));

  const debouncedSearch = useDebounced(search, 150);
  // Query keys are compared by value, so a fresh object each render is fine.
  const query: RecipeQuery = {
    search: debouncedSearch,
    category_ids: categoryIds,
    tag_ids: tagIds,
    sort: sort.sort,
    descending: sort.descending,
  };
  const recipes = useQuery({
    queryKey: queryKeys.recipeList(query),
    queryFn: () => api.listRecipes(query),
    placeholderData: keepPreviousData,
  });

  const filtering = search.trim() !== "" || categoryIds.length > 0 || tagIds.length > 0;

  // Cmd/Ctrl+F focuses the search box.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f") {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const update = (fn: (p: URLSearchParams) => void) =>
    setParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        fn(next);
        return next;
      },
      { replace: true },
    );

  const setSearch = (q: string) => update((p) => (q ? p.set("q", q) : p.delete("q")));

  const toggle = (key: "c" | "t", id: number) =>
    update((p) => {
      const current = p.getAll(key);
      p.delete(key);
      const has = current.includes(String(id));
      for (const v of current) if (v !== String(id)) p.append(key, v);
      if (!has) p.append(key, String(id));
    });

  const clearAll = () => setParams(new URLSearchParams(), { replace: true });

  const changeView = (v: ViewMode) => {
    setView(v);
    remember(VIEW_KEY, v);
  };

  const changeSort = (next: SortPref) => {
    setSort(next);
    remember(SORT_KEY, next);
  };

  const activeFilters = [
    ...categoryIds.map((id) => ({
      key: "c" as const,
      id,
      name: categories.data?.find((l) => l.id === id)?.name ?? "…",
    })),
    ...tagIds.map((id) => ({
      key: "t" as const,
      id,
      name: tags.data?.find((l) => l.id === id)?.name ?? "…",
    })),
  ];

  const count = recipes.data?.length ?? 0;

  return (
    <div className="library">
      <aside className="sidebar" aria-label="Filters">
        <FilterGroup
          title="Categories"
          labels={categories.data}
          selected={categoryIds}
          onToggle={(id) => toggle("c", id)}
        />
        <FilterGroup
          title="Tags"
          labels={tags.data}
          selected={tagIds}
          onToggle={(id) => toggle("t", id)}
          isTag
        />
        <Link to="/organize" className="button ghost small">
          <Settings2 size={14} aria-hidden /> Manage categories &amp; tags
        </Link>
        <div className="filter-group share-links">
          <h2>Share</h2>
          <Link to="/share" className="button small">
            <Share2 size={14} aria-hidden /> Share recipes…
          </Link>
          <Link to="/share/import" className="button small">
            <FileDown size={14} aria-hidden /> Import shared recipes…
          </Link>
        </div>
      </aside>

      <section className="library-main">
        <div className="toolbar">
          <h1>Recipes {recipes.data && <span className="count">{count}</span>}</h1>
          <div className="actions">
            <div className="search">
              <Search size={16} aria-hidden />
              <input
                ref={searchRef}
                type="search"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") setSearch("");
                }}
                placeholder="Search recipes"
                aria-label="Search recipes"
                title="Search (⌘/Ctrl+F)"
              />
            </div>
            <div className="sort">
              <select
                value={sort.sort}
                aria-label="Sort by"
                onChange={(e) => {
                  const s = e.target.value as RecipeSort;
                  // Natural direction per key: A–Z for titles, newest/best first otherwise.
                  changeSort({ sort: s, descending: s !== "title" });
                }}
              >
                {SORTS.map((s) => (
                  <option key={s.value} value={s.value}>
                    {s.label}
                  </option>
                ))}
              </select>
              <button
                type="button"
                className="button icon"
                onClick={() => changeSort({ ...sort, descending: !sort.descending })}
                title={sort.descending ? "Descending" : "Ascending"}
                aria-label={sort.descending ? "Sort descending" : "Sort ascending"}
              >
                {sort.descending ? (
                  <ArrowDownWideNarrow size={16} aria-hidden />
                ) : (
                  <ArrowUpNarrowWide size={16} aria-hidden />
                )}
              </button>
            </div>
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
        </div>

        {activeFilters.length > 0 && (
          <div className="active-filters">
            <span className="muted">Showing recipes with all of:</span>
            {activeFilters.map((f) => (
              <button
                key={`${f.key}${f.id}`}
                className={`chip removable ${f.key === "t" ? "tag" : ""}`}
                onClick={() => toggle(f.key, f.id)}
                aria-label={`Remove filter ${f.name}`}
              >
                {f.key === "t" ? `#${f.name}` : f.name} <X size={12} aria-hidden />
              </button>
            ))}
            <button className="link" onClick={clearAll}>
              Clear all
            </button>
          </div>
        )}

        {recipes.isError && <ErrorBanner message={errorMessage(recipes.error)} />}
        {recipes.data?.length === 0 &&
          (filtering ? (
            <div className="empty">
              <p>No recipes match.</p>
              <button className="button" onClick={clearAll}>
                Clear search and filters
              </button>
            </div>
          ) : (
            <div className="empty">
              <p>No recipes yet.</p>
              <Link to="/recipes/new" className="button primary">
                Add your first recipe
              </Link>
            </div>
          ))}
        {recipes.data && recipes.data.length > 0 && (
          <ul className={view === "grid" ? "recipe-grid" : "recipe-list"}>
            {recipes.data.map((r) => (
              <RecipeCard key={r.id} recipe={r} />
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

function FilterGroup({
  title,
  labels,
  selected,
  onToggle,
  isTag = false,
}: {
  title: string;
  labels: Label[] | undefined;
  selected: number[];
  onToggle: (id: number) => void;
  isTag?: boolean;
}) {
  // Unused labels can't match anything, so only show them while selected.
  const shown = (labels ?? []).filter((l) => l.recipe_count > 0 || selected.includes(l.id));
  return (
    <div className="filter-group">
      <h2>{title}</h2>
      {labels && shown.length === 0 && <p className="muted small">None yet</p>}
      <ul>
        {shown.map((l) => {
          const on = selected.includes(l.id);
          return (
            <li key={l.id}>
              <button
                className={on ? "active" : ""}
                aria-pressed={on}
                onClick={() => onToggle(l.id)}
              >
                <span className="name">{isTag ? `#${l.name}` : l.name}</span>
                <span className="count">{l.recipe_count}</span>
              </button>
            </li>
          );
        })}
      </ul>
    </div>
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
