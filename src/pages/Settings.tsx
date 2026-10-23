import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { RefreshCw } from "lucide-react";
import { api, errorMessage, events, queryKeys } from "../api";
import type { ToolProgress } from "../bindings/ToolProgress";
import { ErrorBanner } from "../components/common";

export default function Settings() {
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
    <div className="settings">
      <div className="toolbar">
        <h1>Settings</h1>
      </div>

      <section>
        <h2>Video downloader</h2>
        <p className="muted">
          Recipio downloads YouTube videos with its own copies of yt-dlp, ffmpeg and Deno. They are
          fetched from their official release pages the first time you download a video, and each
          file is checked against the publisher’s SHA-256 checksum before it is used.
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
    </div>
  );
}
