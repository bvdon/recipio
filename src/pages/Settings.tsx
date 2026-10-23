import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { ask, open, save } from "@tauri-apps/plugin-dialog";
import { Archive, ArchiveRestore, RefreshCw, X } from "lucide-react";
import { api, errorMessage, events, queryKeys, type AppError } from "../api";
import type { BackupProgress } from "../bindings/BackupProgress";
import type { ToolProgress } from "../bindings/ToolProgress";
import { ErrorBanner } from "../components/common";

export default function Settings() {
  return (
    <div className="settings">
      <div className="toolbar">
        <h1>Settings</h1>
      </div>
      <BackupSection />
      <VideoToolsSection />
    </div>
  );
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

function formatBytes(n: number): string {
  if (n === 0) return "0 KB";
  if (n < 1e6) return `${Math.max(1, Math.round(n / 1e3))} KB`;
  if (n < 1e9) return `${Math.round(n / 1e6)} MB`;
  return `${(n / 1e9).toFixed(1)} GB`;
}

const isCancelled = (e: unknown) => (e as AppError | null)?.kind === "cancelled";

function BackupSection() {
  const queryClient = useQueryClient();
  const stats = useQuery({ queryKey: queryKeys.libraryStats, queryFn: api.libraryStats });
  const [includeVideos, setIncludeVideos] = useState(true);
  const [progress, setProgress] = useState<BackupProgress | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const [inspectError, setInspectError] = useState<string | null>(null);

  useEffect(() => {
    const unlisten = listen<BackupProgress>(events.backupProgress, (e) => setProgress(e.payload));
    return () => void unlisten.then((stop) => stop());
  }, []);

  const reset = () => {
    setResult(null);
    setInspectError(null);
    setProgress(null);
  };

  const exportLib = useMutation({
    mutationFn: ({ path, videos }: { path: string; videos: boolean }) =>
      api.exportBackup(path, videos),
    onMutate: reset,
    onSuccess: (_, { path }) => setResult(`Backup saved as ${path.split(/[\\/]/).pop()}.`),
    onError: (e) => {
      if (isCancelled(e)) setResult("Backup cancelled.");
    },
    onSettled: () => setProgress(null),
  });

  const restore = useMutation({
    mutationFn: api.restoreBackup,
    onMutate: reset,
    onSuccess: (n) => {
      setResult(`Restored ${plural(n, "recipe")}.`);
      // Everything changed: recipes, labels, video states.
      void queryClient.invalidateQueries();
    },
    onSettled: () => setProgress(null),
  });

  const busy = exportLib.isPending || restore.isPending;
  const s = stats.data;

  const startExport = async () => {
    const today = new Date().toLocaleDateString("sv"); // YYYY-MM-DD
    const path = await save({
      defaultPath: `Recipio backup ${today}.zip`,
      filters: [{ name: "Recipio backup", extensions: ["zip"] }],
    });
    if (path) exportLib.mutate({ path, videos: includeVideos });
  };

  const startRestore = async () => {
    const path = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "Recipio backup", extensions: ["zip"] }],
    });
    if (!path) return;
    reset();
    let summary;
    try {
      summary = await api.inspectBackup(path);
    } catch (e) {
      setInspectError(errorMessage(e));
      return;
    }
    const made = summary.created_at
      ? ` from ${new Date(summary.created_at).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}`
      : "";
    const current = s?.recipes ?? 0;
    const ok = await ask(
      `The backup${made} has ${plural(summary.recipes, "recipe")}, ${plural(summary.photos, "photo")} and ${plural(summary.videos, "video")}.\n\n` +
        `Restoring it replaces your whole library: your current ${plural(current, "recipe")}, with their photos and videos, will be deleted. This can't be undone, so export a backup first if you might want them back.`,
      {
        title: "Replace your library?",
        kind: "warning",
        okLabel: "Replace library",
        cancelLabel: "Cancel",
      },
    );
    if (ok) restore.mutate(path);
  };

  const error = [exportLib.error, restore.error].find((e) => e && !isCancelled(e));

  return (
    <section>
      <h2>Backup</h2>
      <p className="muted">
        A backup is a single .zip file with all your recipes, categories, tags, photos and
        (optionally) downloaded videos. Restoring a backup replaces everything in the library.
      </p>
      {s && (
        <p>
          Your library: {plural(s.recipes, "recipe")}, {plural(s.photos, "photo")} (
          {formatBytes(s.photo_bytes)}), {plural(s.videos, "video")}
          {s.videos > 0 && ` (${formatBytes(s.video_bytes)})`}.
        </p>
      )}
      {s && s.videos > 0 && (
        <label className="check">
          <input
            type="checkbox"
            checked={includeVideos}
            disabled={busy}
            onChange={(e) => setIncludeVideos(e.target.checked)}
          />
          Include downloaded videos ({formatBytes(s.video_bytes)})
        </label>
      )}
      <div className="backup-actions">
        <button
          type="button"
          className="button primary"
          disabled={busy}
          onClick={() => void startExport()}
        >
          <Archive size={16} aria-hidden /> Export library…
        </button>
        <button
          type="button"
          className="button"
          disabled={busy}
          onClick={() => void startRestore()}
        >
          <ArchiveRestore size={16} aria-hidden /> Restore from backup…
        </button>
      </div>
      {busy && (
        <div className="video-status" aria-live="polite">
          <div className="video-progress-head">
            <span>{exportLib.isPending ? "Saving backup…" : "Restoring backup…"}</span>
            {exportLib.isPending && (
              <button
                type="button"
                className="button small"
                onClick={() => void api.cancelBackup()}
              >
                <X size={14} aria-hidden /> Cancel
              </button>
            )}
          </div>
          {progress && progress.total > 0 ? (
            <progress max={progress.total} value={progress.done} aria-label="Backup progress" />
          ) : (
            <progress aria-label="Backup progress" />
          )}
          {progress && progress.total > 0 && (
            <p className="hint">
              {formatBytes(progress.done)} of {formatBytes(progress.total)}
            </p>
          )}
        </div>
      )}
      {result && <p className="hint">{result}</p>}
      {inspectError && <ErrorBanner message={inspectError} />}
      {error && <ErrorBanner message={errorMessage(error)} />}
    </section>
  );
}

function VideoToolsSection() {
  const queryClient = useQueryClient();
  const tools = useQuery({ queryKey: queryKeys.tools, queryFn: api.toolsStatus });
  const [progress, setProgress] = useState<ToolProgress | null>(null);
  const [result, setResult] = useState<string | null>(null);

  useEffect(() => {
    const unlisten = listen<ToolProgress>(events.toolsProgress, (e) => setProgress(e.payload));
    return () => void unlisten.then((stop) => stop());
  }, []);

  const installed = tools.data?.every((t) => t.version !== null) ?? false;

  const update = useMutation({
    mutationFn: () => api.installTools(true),
    onMutate: () => {
      setResult(null);
      setProgress(null);
    },
    onSuccess: (changed) => {
      const names = tools.data?.filter((t) => changed.includes(t.package)).map((t) => t.name);
      setResult(
        names?.length ? `Updated ${names.join(", ")}.` : "Everything is already up to date.",
      );
    },
    onSettled: () => {
      setProgress(null);
      void queryClient.invalidateQueries({ queryKey: queryKeys.tools });
    },
  });

  return (
    <section>
      <h2>Video downloader</h2>
      <p className="muted">
        Recipio downloads YouTube videos with its own copies of yt-dlp, ffmpeg and Deno. They are
        fetched from their official release pages the first time you download a video, and each file
        is checked against the publisher’s SHA-256 checksum before it is used.
      </p>
      {tools.isError && <ErrorBanner message={errorMessage(tools.error)} />}
      <table className="tools-table">
        <tbody>
          {tools.data?.map((t) => (
            <tr key={t.package}>
              <th scope="row">{t.name}</th>
              <td>{t.version ?? <span className="muted">Not installed</span>}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="row-actions">
        <button
          type="button"
          className="button primary"
          disabled={update.isPending}
          onClick={() => update.mutate()}
        >
          <RefreshCw size={16} aria-hidden className={update.isPending ? "spin" : ""} />
          {installed ? "Update downloader" : "Install downloader"}
        </button>
      </div>
      {update.isPending && (
        <div className="video-status" aria-live="polite">
          <p className="hint">
            {progress
              ? `Downloading ${progress.name}…`
              : installed
                ? "Checking for updates…"
                : "Starting…"}
          </p>
          {progress?.total ? (
            <progress max={progress.total} value={progress.downloaded} />
          ) : (
            <progress />
          )}
        </div>
      )}
      {result && <p className="hint">{result}</p>}
      {update.isError && <ErrorBanner message={errorMessage(update.error)} />}
      <p className="hint">
        Video downloading is meant for personal, offline use of videos you have access to.
      </p>
    </section>
  );
}
