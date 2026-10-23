import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router";
import { keepPreviousData, useMutation, useQuery } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import { ArrowLeft, Search, Share2, Video, X } from "lucide-react";
import { api, errorMessage, events, mediaUrl, queryKeys } from "../api";
import type { BackupProgress } from "../bindings/BackupProgress";
import type { RecipeQuery } from "../bindings/RecipeQuery";
import {
  ErrorBanner,
  fileName,
  formatBytes,
  isCancelled,
  PhotoPlaceholder,
  plural,
} from "../components/common";

const ALL: RecipeQuery = {
  search: "",
  category_ids: [],
  tag_ids: [],
  sort: "title",
  descending: false,
};

/** Pick recipes and save them to a `.recipio` file another Recipio user can import. */
export default function ShareRecipes() {
  const recipes = useQuery({
    queryKey: queryKeys.recipeList(ALL),
    queryFn: () => api.listRecipes(ALL),
  });
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [includeVideos, setIncludeVideos] = useState(false);
  const [progress, setProgress] = useState<BackupProgress | null>(null);
  const [result, setResult] = useState<string | null>(null);

  const shown = useMemo(() => {
    const words = filter.toLowerCase().split(/\s+/).filter(Boolean);
    return (recipes.data ?? []).filter((r) =>
      words.every((w) => r.title.toLowerCase().includes(w)),
    );
  }, [recipes.data, filter]);

  const ids = useMemo(() => [...selected].sort(), [selected]);
  const stats = useQuery({
    queryKey: [...queryKeys.libraryStats, ids],
    queryFn: () => api.libraryStats(ids),
    enabled: ids.length > 0,
    placeholderData: keepPreviousData,
  });

  useEffect(() => {
    const unlisten = listen<BackupProgress>(events.backupProgress, (e) => setProgress(e.payload));
    return () => void unlisten.then((stop) => stop());
  }, []);

  const share = useMutation({
    mutationFn: ({ path, videos }: { path: string; videos: boolean }) =>
      api.shareRecipes(path, ids, videos),
    onMutate: () => {
      setResult(null);
      setProgress(null);
    },
    onSuccess: (_, { path }) =>
      setResult(
        `Saved ${fileName(path)}. Send it to your friend; they add it with “Import shared recipes”.`,
      ),
    onError: (e) => {
      if (isCancelled(e)) setResult("Cancelled.");
    },
    onSettled: () => setProgress(null),
  });

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

  const start = async () => {
    const first = recipes.data?.find((r) => r.id === ids[0])?.title;
    const name =
      ids.length === 1 && first ? first.replace(/[\\/:*?"<>|]/g, "") : `${ids.length} recipes`;
    const path = await save({
      defaultPath: `${name}.recipio`,
      filters: [{ name: "Recipio recipes", extensions: ["recipio"] }],
    });
    if (path) share.mutate({ path, videos: includeVideos && (stats.data?.videos ?? 0) > 0 });
  };

  const s = ids.length > 0 ? stats.data : undefined;
  const error = share.isError && !isCancelled(share.error) ? errorMessage(share.error) : null;

  return (
    <div className="share-page">
      <div className="toolbar">
        <Link to="/" className="button ghost">
          <ArrowLeft size={16} aria-hidden /> Recipes
        </Link>
      </div>
      <h1>Share recipes</h1>
      <p className="muted">
        Choose recipes to save in a file you can send to a friend who uses Recipio (by email,
        AirDrop, a USB stick…). They add it with <strong>Import shared recipes</strong>. Photos,
        notes and video bookmarks go along; your star ratings don’t.
      </p>

      {recipes.isError && <ErrorBanner message={errorMessage(recipes.error)} />}

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
        {recipes.data && shown.length === 0 && (
          <li className="muted empty">
            {recipes.data.length ? "No recipes match." : "No recipes yet."}
          </li>
        )}
      </ul>

      <div className="share-footer">
        <div>
          {s ? (
            <>
              {plural(s.recipes, "recipe")}, {plural(s.photos, "photo")} (
              {formatBytes(s.photo_bytes)})
              {s.videos > 0 && (
                <label className="check">
                  <input
                    type="checkbox"
                    checked={includeVideos}
                    disabled={share.isPending}
                    onChange={(e) => setIncludeVideos(e.target.checked)}
                  />
                  Include {plural(s.videos, "downloaded video")} ({formatBytes(s.video_bytes)}) —
                  otherwise your friend can download them from the YouTube links
                </label>
              )}
            </>
          ) : (
            <span className="muted">Select the recipes to share.</span>
          )}
        </div>
        <button
          type="button"
          className="button primary"
          disabled={ids.length === 0 || share.isPending}
          onClick={() => void start()}
        >
          <Share2 size={16} aria-hidden /> Save share file…
        </button>
      </div>

      {share.isPending && (
        <div className="video-status" aria-live="polite">
          <div className="video-progress-head">
            <span>Saving…</span>
            <button type="button" className="button small" onClick={() => void api.cancelBackup()}>
              <X size={14} aria-hidden /> Cancel
            </button>
          </div>
          {progress && progress.total > 0 ? (
            <progress max={progress.total} value={progress.done} aria-label="Progress" />
          ) : (
            <progress aria-label="Progress" />
          )}
        </div>
      )}
      {result && <p className="hint">{result}</p>}
      {error && <ErrorBanner message={error} />}
    </div>
  );
}
