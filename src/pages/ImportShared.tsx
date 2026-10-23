import { useEffect, useState } from "react";
import { Link } from "react-router";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { ArrowLeft, FileDown, Image, Video } from "lucide-react";
import { api, errorMessage, events, queryKeys } from "../api";
import type { BackupProgress } from "../bindings/BackupProgress";
import { ErrorBanner, fileName, plural } from "../components/common";

/**
 * Add recipes from a `.recipio` file a friend shared. Only ever adds: recipes already in
 * the library are unticked, and come in as copies if ticked.
 */
export default function ImportShared() {
  const queryClient = useQueryClient();
  const [path, setPath] = useState<string | null>(null);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [added, setAdded] = useState<number | null>(null);
  const [progress, setProgress] = useState<BackupProgress | null>(null);

  const preview = useMutation({
    mutationFn: api.previewShared,
    // Start with everything new ticked.
    onSuccess: (p) => setSelected(new Set(p.recipes.filter((r) => !r.in_library).map((r) => r.id))),
  });

  useEffect(() => {
    const unlisten = listen<BackupProgress>(events.backupProgress, (e) => setProgress(e.payload));
    return () => void unlisten.then((stop) => stop());
  }, []);

  const add = useMutation({
    mutationFn: () => api.addSharedRecipes(path!, [...selected]),
    onMutate: () => setProgress(null),
    onSuccess: (n) => {
      setAdded(n);
      void queryClient.invalidateQueries({ queryKey: queryKeys.recipes });
      void queryClient.invalidateQueries({ queryKey: queryKeys.allLabels });
      void queryClient.invalidateQueries({ queryKey: queryKeys.libraryStats });
    },
    onSettled: () => setProgress(null),
  });

  const choose = async () => {
    const chosen = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "Recipio recipes", extensions: ["recipio", "zip"] }],
    });
    if (!chosen) return;
    setAdded(null);
    add.reset();
    setPath(chosen);
    preview.mutate(chosen);
  };

  const toggle = (id: string) =>
    setSelected((s) => {
      const next = new Set(s);
      if (!next.delete(id)) next.add(id);
      return next;
    });

  const p = preview.data;
  const shared = p?.created_at
    ? new Date(p.created_at).toLocaleDateString(undefined, { dateStyle: "medium" })
    : null;

  return (
    <div className="share-page">
      <div className="toolbar">
        <Link to="/" className="button ghost">
          <ArrowLeft size={16} aria-hidden /> Recipes
        </Link>
      </div>
      <h1>Import shared recipes</h1>
      <p className="muted">
        Add recipes a friend shared with you as a <strong>.recipio</strong> file. Nothing already in
        your library is changed or removed.
      </p>

      {added !== null ? (
        <div className="import-done" role="status">
          <p>Added {plural(added, "recipe")} to your library.</p>
          <div className="row-actions">
            <Link to="/" className="button primary">
              Go to recipes
            </Link>
            <button type="button" className="button" onClick={() => void choose()}>
              Import another file…
            </button>
          </div>
        </div>
      ) : (
        <>
          <div className="row-actions">
            <button
              type="button"
              className={p ? "button" : "button primary"}
              onClick={() => void choose()}
              disabled={add.isPending}
            >
              <FileDown size={16} aria-hidden /> {p ? "Choose a different file…" : "Choose file…"}
            </button>
          </div>
          {preview.isError && <ErrorBanner message={errorMessage(preview.error)} />}

          {p && path && (
            <>
              <h2 className="share-file">
                {fileName(path)}
                <span className="muted">
                  {" "}
                  · {plural(p.recipes.length, "recipe")}
                  {shared && ` · shared ${shared}`}
                </span>
              </h2>
              <ul className="pick-list">
                {p.recipes.map((r) => (
                  <li key={r.id} className={selected.has(r.id) ? "on" : undefined}>
                    <label>
                      <input
                        type="checkbox"
                        checked={selected.has(r.id)}
                        onChange={() => toggle(r.id)}
                      />
                      <span className="pick-title">{r.title}</span>
                      {r.in_library && (
                        <span className="chip">
                          {selected.has(r.id)
                            ? "Already in your library — will be added as a copy"
                            : "Already in your library"}
                        </span>
                      )}
                      {r.has_photo && (
                        <Image size={16} aria-label="Has a photo" className="muted" />
                      )}
                      {r.has_video && (
                        <Video size={16} aria-label="Has a video" className="muted" />
                      )}
                    </label>
                  </li>
                ))}
              </ul>
              <div className="share-footer">
                <span className="muted">{plural(selected.size, "recipe")} selected</span>
                <button
                  type="button"
                  className="button primary"
                  disabled={selected.size === 0 || add.isPending}
                  onClick={() => add.mutate()}
                >
                  Add {plural(selected.size, "recipe")} to my library
                </button>
              </div>
              {add.isPending && (
                <div className="video-status" aria-live="polite">
                  <span>Adding recipes…</span>
                  {progress && progress.total > 0 ? (
                    <progress max={progress.total} value={progress.done} aria-label="Progress" />
                  ) : (
                    <progress aria-label="Progress" />
                  )}
                </div>
              )}
              {add.isError && <ErrorBanner message={errorMessage(add.error)} />}
            </>
          )}
        </>
      )}
    </div>
  );
}
