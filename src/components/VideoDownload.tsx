import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { ask } from "@tauri-apps/plugin-dialog";
import { CheckCircle2, Download, RotateCcw, Trash2, X } from "lucide-react";
import { api, errorMessage, events, queryKeys } from "../api";
import type { VideoInfo } from "../bindings/VideoInfo";
import type { VideoJob } from "../bindings/VideoJob";
import type { VideoStage } from "../bindings/VideoStage";
import type { VideoStatus } from "../bindings/VideoStatus";
import { ErrorBanner } from "./common";

const RUNNING: VideoStage[] = ["installing_tools", "preparing", "downloading", "transcoding"];

const STAGE_LABEL: Partial<Record<VideoStage, string>> = {
  installing_tools: "Setting up the video downloader (first time only)…",
  preparing: "Getting video details…",
  downloading: "Downloading video…",
  transcoding: "Converting video…",
};

/** The saved recipe's video, as far as the page knows. */
export type SavedVideo = {
  status: VideoStatus;
  /** URL of the saved video, if any. */
  url: string | null;
};

/**
 * Live state of a recipe's background download (seeded from Rust, then kept current by
 * `video://job` events), plus start/cancel/remove actions.
 */
export function useVideoDownload(recipeId: string, onDone?: () => void) {
  const queryClient = useQueryClient();
  const key = queryKeys.videoJob(recipeId);
  const job = useQuery({ queryKey: key, queryFn: () => api.getVideoJob(recipeId) });
  // Set after "Remove video" so the panel stops showing the old video before a refetch.
  const [removed, setRemoved] = useState(false);
  const onDoneRef = useRef(onDone);
  useEffect(() => {
    onDoneRef.current = onDone;
  });

  useEffect(() => {
    const unlisten = listen<VideoJob>(events.videoJob, ({ payload }) => {
      if (payload.recipe_id !== recipeId) return;
      queryClient.setQueryData(queryKeys.videoJob(recipeId), payload);
      if (payload.stage === "done") {
        void queryClient.invalidateQueries({ queryKey: queryKeys.recipes });
        // Marked stale only: an open editor must not reload its form underneath the user.
        void queryClient.invalidateQueries({
          queryKey: queryKeys.recipe(recipeId),
          refetchType: "none",
        });
        onDoneRef.current?.();
      }
    });
    return () => void unlisten.then((stop) => stop());
  }, [recipeId, queryClient]);

  const start = useMutation({
    mutationFn: (url: string) => api.startVideoDownload(recipeId, url),
    onSuccess: (j) => {
      setRemoved(false);
      queryClient.setQueryData(key, j);
    },
  });

  const remove = useMutation({
    mutationFn: () => api.removeVideo(recipeId),
    onSuccess: () => {
      setRemoved(true);
      queryClient.setQueryData(key, null);
      void queryClient.invalidateQueries({ queryKey: queryKeys.recipes });
      void queryClient.invalidateQueries({
        queryKey: queryKeys.recipe(recipeId),
        refetchType: "none",
      });
      onDoneRef.current?.();
    },
  });

  const dismiss = () => {
    void api.dismissVideoJob(recipeId);
    queryClient.setQueryData(key, null);
  };

  return {
    job: job.data ?? null,
    removed,
    start,
    remove,
    dismiss,
    cancel: () => void api.cancelVideoDownload(recipeId),
  };
}

export type VideoDownload = ReturnType<typeof useVideoDownload>;

export function isRunning(job: VideoJob | null): boolean {
  return job !== null && RUNNING.includes(job.stage);
}

/**
 * Download status and actions for a recipe's video: progress with Cancel, errors with
 * Retry, Re-download when the file is missing, Remove when present.
 */
export function VideoStatusPanel({
  dl,
  saved,
  url,
}: {
  dl: VideoDownload;
  saved: SavedVideo;
  /** The YouTube URL currently entered (editor) or saved (recipe page). */
  url: string;
}) {
  const { job } = dl;
  const valid = useQuery({
    queryKey: ["check-youtube-url", url.trim()],
    queryFn: () => api.checkYoutubeUrl(url.trim()),
    enabled: url.trim() !== "",
    staleTime: Infinity,
  });
  const urlOk = url.trim() !== "" && valid.data === true;
  const status: VideoStatus = dl.removed ? "none" : saved.status;
  const startError = dl.start.isError ? errorMessage(dl.start.error) : null;
  const removeError = dl.remove.isError ? errorMessage(dl.remove.error) : null;

  const download = (label: string, icon = <Download size={16} aria-hidden />) => (
    <button
      type="button"
      className="button"
      disabled={!urlOk || dl.start.isPending}
      onClick={() => dl.start.mutate(url.trim())}
    >
      {icon} {label}
    </button>
  );

  const confirmRemove = async () => {
    const ok = await ask(
      "Delete the downloaded video? The YouTube link is kept, so you can download it again later.",
      { title: "Remove video", kind: "warning", okLabel: "Delete video", cancelLabel: "Cancel" },
    );
    if (ok) dl.remove.mutate();
  };

  let body: React.ReactNode = null;
  if (job && isRunning(job)) {
    body = (
      <>
        <div className="video-progress-head">
          <span>{STAGE_LABEL[job.stage]}</span>
          <button type="button" className="button small" onClick={dl.cancel}>
            <X size={14} aria-hidden /> Cancel
          </button>
        </div>
        {job.percent === null ? (
          <progress aria-label="Download progress" />
        ) : (
          <progress max={100} value={job.percent} aria-label="Download progress" />
        )}
        {job.detail && <p className="hint">{job.detail}</p>}
      </>
    );
  } else if (job?.stage === "failed") {
    body = (
      <>
        <ErrorBanner message={job.error ?? "The download failed."} />
        <div className="row-actions">
          {download("Try again", <RotateCcw size={16} aria-hidden />)}
          <button type="button" className="button ghost" onClick={dl.dismiss}>
            Dismiss
          </button>
        </div>
      </>
    );
  } else if (job?.stage === "done" || status === "ready") {
    const other = urlOk && url.trim() !== (job?.stage === "done" ? job.url : saved.url);
    body = (
      <div className="video-ready">
        <span className="ok">
          <CheckCircle2 size={16} aria-hidden /> Video saved for offline viewing
        </span>
        <div className="row-actions">
          {other && download("Download this link instead")}
          <button
            type="button"
            className="button ghost"
            disabled={dl.remove.isPending}
            onClick={() => void confirmRemove()}
          >
            <Trash2 size={16} aria-hidden /> Remove video
          </button>
        </div>
      </div>
    );
  } else if (status === "missing") {
    body = (
      <div className="video-ready">
        <span className="muted">The video file is missing.</span>
        {download("Re-download", <RotateCcw size={16} aria-hidden />)}
      </div>
    );
  } else if (status === "failed" && job?.stage !== "cancelled") {
    body = (
      <div className="video-ready">
        <span className="muted">The last download didn't finish.</span>
        {download("Try again", <RotateCcw size={16} aria-hidden />)}
      </div>
    );
  } else if (urlOk) {
    body = (
      <div className="video-ready">
        <span className="muted">
          {job?.stage === "cancelled" ? "Download cancelled." : "Not downloaded yet."}
        </span>
        {download("Download video")}
      </div>
    );
  } else if (url.trim() !== "" && valid.data === false) {
    body = <p className="hint">That doesn't look like a YouTube video link.</p>;
  }

  return (
    <div className="video-status" aria-live="polite">
      {job?.info && <VideoTitle info={job.info} />}
      {body}
      {startError && <ErrorBanner message={startError} />}
      {removeError && <ErrorBanner message={removeError} />}
    </div>
  );
}

function VideoTitle({ info }: { info: VideoInfo }) {
  const d = info.duration_seconds;
  const duration =
    d === null
      ? null
      : d >= 3600
        ? `${Math.floor(d / 3600)}:${pad(Math.floor(d / 60) % 60)}:${pad(Math.floor(d) % 60)}`
        : `${Math.floor(d / 60)}:${pad(Math.floor(d) % 60)}`;
  return (
    <p className="video-title">
      “{info.title}”{duration && <span className="muted"> · {duration}</span>}
    </p>
  );
}

const pad = (n: number) => String(n).padStart(2, "0");

/**
 * The editor's YouTube URL field. Pasting a video link starts the download right away;
 * the first metadata to arrive is handed to `onInfo` (to pre-fill title and photo).
 */
export function VideoField({
  recipeId,
  value,
  onChange,
  saved,
  onInfo,
}: {
  recipeId: string;
  value: string;
  onChange: (url: string) => void;
  saved: SavedVideo;
  onInfo: (info: VideoInfo) => void;
}) {
  const dl = useVideoDownload(recipeId);
  const appliedFor = useRef<string | null>(null);

  // Hand over metadata once per download.
  const { job } = dl;
  useEffect(() => {
    if (job?.info && isRunning(job) && appliedFor.current !== job.url) {
      appliedFor.current = job.url;
      onInfo(job.info);
    }
  }, [job, onInfo]);

  const onPaste = (text: string) => {
    const url = text.trim();
    if (isRunning(job) || url === saved.url) return;
    void api.checkYoutubeUrl(url).then((ok) => {
      if (ok) dl.start.mutate(url);
    });
  };

  return (
    <div className="video-field">
      <label>
        YouTube URL
        <input
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onPaste={(e) => onPaste(e.clipboardData.getData("text/plain"))}
          placeholder="https://www.youtube.com/watch?v=…"
          type="url"
        />
      </label>
      <VideoStatusPanel dl={dl} saved={saved} url={value} />
    </div>
  );
}
