import { useEffect, useRef, useState } from "react";
import { Link } from "react-router";
import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import { FolderOpen, Send, X } from "lucide-react";
import { api, errorMessage, events, queryKeys } from "../api";
import type { BackupProgress } from "../bindings/BackupProgress";
import type { Friend } from "../bindings/Friend";
import type { ShareFormat } from "../bindings/ShareFormat";
import type { ShareOutcome } from "../bindings/ShareOutcome";
import { ErrorBanner, fileName, formatBytes, isCancelled, plural } from "./common";

/** Email services usually refuse attachments above about 20–25 MB. */
const EMAIL_LIMIT = 20e6;
/** Share files carry photos resized to about this size (see `backup::shrink_photo`). */
const SHARED_PHOTO_BYTES = 400e3;

/** Messages/Mail with the attachment and the share menu are macOS features. */
const IS_MAC = navigator.userAgent.includes("Mac");

type Target = { kind: "friend"; id: number } | { kind: "other" } | { kind: "file" };

/**
 * Send recipes to a friend (Messages / Mail on macOS, an email draft elsewhere), through
 * the system share menu, or to a file — as a Recipio file or a PDF.
 */
export default function ShareDialog({
  ids,
  friendId = null,
  onClose,
}: {
  ids: string[];
  friendId?: number | null;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const sendButton = useRef<HTMLButtonElement>(null);
  const queryClient = useQueryClient();
  const friends = useQuery({ queryKey: queryKeys.friends, queryFn: api.listFriends });
  const myName = useQuery({ queryKey: queryKeys.myName, queryFn: api.getMyName });
  const stats = useQuery({
    queryKey: [...queryKeys.libraryStats, [...ids].sort()],
    queryFn: () => api.libraryStats(ids),
    placeholderData: keepPreviousData,
  });
  const [picked, setPicked] = useState<Target | null>(
    friendId !== null ? { kind: "friend", id: friendId } : null,
  );
  const [format, setFormat] = useState<ShareFormat>("recipio");
  const [includeVideos, setIncludeVideos] = useState(false);
  const [note, setNote] = useState("");
  const [name, setName] = useState("");
  const [progress, setProgress] = useState<BackupProgress | null>(null);
  const [outcome, setOutcome] = useState<ShareOutcome | null>(null);

  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  useEffect(() => {
    const unlisten = listen<BackupProgress>(events.backupProgress, (e) => setProgress(e.payload));
    return () => void unlisten.then((stop) => stop());
  }, []);

  const list = friends.data ?? [];
  // Default to the first friend once they're known.
  const target: Target =
    picked ?? (list.length > 0 ? { kind: "friend", id: list[0].id } : { kind: "other" });
  const friend: Friend | undefined =
    target.kind === "friend" ? list.find((f) => f.id === target.id) : undefined;
  const needsName = myName.data === "" && format === "recipio" && target.kind !== "file";

  const s = stats.data;
  const videos = format === "recipio" && includeVideos ? (s?.video_bytes ?? 0) : 0;
  const estimate = s
    ? Math.min(s.photo_bytes, s.photos * SHARED_PHOTO_BYTES) + videos + 20e3 * s.recipes
    : 0;
  const tooBigForEmail = friend?.method === "email" && estimate > EMAIL_LIMIT;

  const send = useMutation({
    mutationFn: async () => {
      if (needsName) {
        await api.setMyName(name);
        void queryClient.invalidateQueries({ queryKey: queryKeys.myName });
      }
      let request: Parameters<typeof api.share>[0]["target"];
      if (target.kind === "file") {
        const ext = format === "pdf" ? "pdf" : "recipio";
        const path = await save({
          defaultPath: `${ids.length === 1 ? "Recipe" : `${ids.length} recipes`}.${ext}`,
          filters: [{ name: format === "pdf" ? "PDF" : "Recipio recipes", extensions: [ext] }],
        });
        if (!path) return null;
        request = { kind: "file", path };
      } else if (target.kind === "other") {
        const r = sendButton.current?.getBoundingClientRect();
        request = {
          kind: "other",
          anchor: { x: r?.x ?? 0, y: r?.y ?? 0, width: r?.width ?? 1, height: r?.height ?? 1 },
        };
      } else {
        request = { kind: "friend", id: target.id };
      }
      return api.share({
        ids,
        format,
        include_videos: includeVideos,
        note,
        target: request,
        locale: navigator.language,
      });
    },
    onMutate: () => setProgress(null),
    onSuccess: (result) => {
      if (!result) return;
      setOutcome(result);
      void queryClient.invalidateQueries({ queryKey: queryKeys.allHistory });
    },
    onSettled: () => setProgress(null),
  });

  const sendLabel =
    target.kind === "file"
      ? "Save file…"
      : target.kind === "other"
        ? "Share…"
        : friend?.method === "email"
          ? "Open email"
          : IS_MAC
            ? "Open in Messages"
            : "Show file";

  return (
    <dialog
      ref={dialog}
      className="share-dialog"
      aria-labelledby="share-title"
      onCancel={(e) => {
        e.preventDefault();
        if (!send.isPending) onClose();
      }}
    >
      <h2 id="share-title">Share {ids.length === 1 ? "recipe" : plural(ids.length, "recipe")}</h2>

      {outcome ? (
        <Done outcome={outcome} friend={friend} onClose={onClose} />
      ) : (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            send.mutate();
          }}
        >
          <fieldset className="share-targets">
            <legend>Send to</legend>
            {list.map((f) => (
              <label key={f.id} className="radio">
                <input
                  type="radio"
                  name="target"
                  checked={target.kind === "friend" && target.id === f.id}
                  onChange={() => setPicked({ kind: "friend", id: f.id })}
                />
                <span>
                  {f.name}{" "}
                  <span className="muted">
                    · {f.method === "messages" ? "Messages" : "Email"} · {f.address}
                  </span>
                </span>
              </label>
            ))}
            {friends.data && list.length === 0 && (
              <p className="hint">
                No friends yet. <Link to="/friends">Add a friend</Link> to send in one click.
              </p>
            )}
            <label className="radio">
              <input
                type="radio"
                name="target"
                checked={target.kind === "other"}
                onChange={() => setPicked({ kind: "other" })}
              />
              <span>
                Other…{" "}
                <span className="muted">
                  {IS_MAC
                    ? "· AirDrop, Messages, Mail, Notes…"
                    : "· shows the file so you can send it any way you like"}
                </span>
              </span>
            </label>
            <label className="radio">
              <input
                type="radio"
                name="target"
                checked={target.kind === "file"}
                onChange={() => setPicked({ kind: "file" })}
              />
              <span>Save a file</span>
            </label>
          </fieldset>

          <fieldset className="share-format">
            <legend>Send as</legend>
            <label className="radio">
              <input
                type="radio"
                name="format"
                checked={format === "recipio"}
                onChange={() => setFormat("recipio")}
              />
              <span>
                Recipio file <span className="muted">· for friends who use Recipio</span>
              </span>
            </label>
            <label className="radio">
              <input
                type="radio"
                name="format"
                checked={format === "pdf"}
                onChange={() => setFormat("pdf")}
              />
              <span>
                PDF <span className="muted">· anyone can read or print it (no video)</span>
              </span>
            </label>
            {format === "recipio" && (s?.videos ?? 0) > 0 && (
              <label className="check">
                <input
                  type="checkbox"
                  checked={includeVideos}
                  onChange={(e) => setIncludeVideos(e.target.checked)}
                />
                Include {plural(s!.videos, "downloaded video")} ({formatBytes(s!.video_bytes)}) —
                otherwise they can download them from the YouTube links
              </label>
            )}
          </fieldset>

          {needsName && (
            <label>
              Your name <span className="muted">(shown to friends in what you send)</span>
              <input value={name} onChange={(e) => setName(e.target.value)} required />
            </label>
          )}
          {target.kind !== "file" && (
            <label>
              Note <span className="muted">(optional)</span>
              <textarea
                value={note}
                onChange={(e) => setNote(e.target.value)}
                rows={2}
                placeholder="Try this one, it's great!"
              />
            </label>
          )}

          {s && (
            <p className="hint">
              About {formatBytes(estimate)}.
              {tooBigForEmail &&
                " That may be too big for email — send fewer recipes, leave out videos, or use AirDrop."}
            </p>
          )}
          {send.isPending && (
            <div className="video-status" aria-live="polite">
              <span>Preparing…</span>
              {progress && progress.total > 0 ? (
                <progress max={progress.total} value={progress.done} aria-label="Progress" />
              ) : (
                <progress aria-label="Progress" />
              )}
            </div>
          )}
          {send.isError && !isCancelled(send.error) && (
            <ErrorBanner message={errorMessage(send.error)} />
          )}

          <div className="actions dialog-actions">
            <button type="button" className="button" onClick={onClose} disabled={send.isPending}>
              Cancel
            </button>
            <button
              ref={sendButton}
              type="submit"
              className="button primary"
              disabled={send.isPending || (needsName && name.trim() === "")}
            >
              <Send size={16} aria-hidden /> {sendLabel}
            </button>
          </div>
        </form>
      )}
    </dialog>
  );
}

function Done({
  outcome,
  friend,
  onClose,
}: {
  outcome: ShareOutcome;
  friend: Friend | undefined;
  onClose: () => void;
}) {
  const who = friend?.name ?? "your friend";
  const text: Record<ShareOutcome["handoff"], string> = {
    messages: `Messages is open with the file for ${who}. Add a note if you like, then press Send.`,
    mail: `A new email to ${who} is open in Mail with the file attached. Press Send when ready.`,
    share_menu: "Pick how to send it from the menu.",
    email_draft: `A new email to ${who} is open, and the file is shown in its folder — drag it into the email, then send.`,
    folder: "The file is shown in its folder. Attach it to a message or email to send it.",
    saved: `Saved ${fileName(outcome.file)}.`,
  };
  return (
    <div className="share-done" role="status">
      <p>{text[outcome.handoff]}</p>
      <p className="hint">
        {fileName(outcome.file)} · {formatBytes(outcome.bytes)}
      </p>
      <div className="actions dialog-actions">
        <button type="button" className="button" onClick={() => void api.revealFile(outcome.file)}>
          <FolderOpen size={16} aria-hidden /> Show file
        </button>
        <button type="button" className="button primary" onClick={onClose}>
          <X size={16} aria-hidden /> Done
        </button>
      </div>
    </div>
  );
}
