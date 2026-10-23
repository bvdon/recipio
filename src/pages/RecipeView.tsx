import { useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ask, save } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowLeft,
  BookmarkPlus,
  Clock,
  CookingPot,
  ExternalLink,
  Pencil,
  Printer,
  Send,
  Trash2,
  Users,
  X,
} from "lucide-react";
import { api, errorMessage, mediaUrl, queryKeys } from "../api";
import type { Label } from "../bindings/Label";
import type { Recipe } from "../bindings/Recipe";
import type { VideoBookmark } from "../bindings/VideoBookmark";
import { ErrorBanner, formatMinutes, Stars } from "../components/common";
import { isRunning, useVideoDownload, VideoStatusPanel } from "../components/VideoDownload";
import ShareDialog from "../components/ShareDialog";

export default function RecipeView() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const recipe = useQuery({ queryKey: queryKeys.recipe(id), queryFn: () => api.getRecipe(id) });
  const categories = useQuery({
    queryKey: queryKeys.labels("category"),
    queryFn: () => api.listLabels("category"),
  });
  const tags = useQuery({
    queryKey: queryKeys.labels("tag"),
    queryFn: () => api.listLabels("tag"),
  });

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

  const [sharing, setSharing] = useState(false);

  // Saves a PDF (text and photo, never the video) and opens it.
  const pdf = useMutation({
    mutationFn: (path: string) => api.exportPdf(id, path, navigator.language),
  });

  const savePdf = async (r: Recipe) => {
    const name = r.title.replace(/[\\/:*?"<>|]/g, "").trim() || "Recipe";
    const path = await save({
      defaultPath: `${name}.pdf`,
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
    if (path) pdf.mutate(path);
  };

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
          <Link to={`/recipes/${r.id}/cook`} className="button primary">
            <CookingPot size={16} aria-hidden /> Start cooking
          </Link>
          <Link to={`/recipes/${r.id}/edit`} className="button">
            <Pencil size={16} aria-hidden /> Edit
          </Link>
          <button
            type="button"
            className="button"
            disabled={pdf.isPending}
            onClick={() => void savePdf(r)}
            title="Save as a PDF to print or share (without the video)"
          >
            <Printer size={16} aria-hidden /> PDF
          </button>
          <button
            type="button"
            className="button"
            onClick={() => setSharing(true)}
            title="Send this recipe to a friend"
          >
            <Send size={16} aria-hidden /> Share
          </button>
          <button className="button danger" onClick={() => void confirmDelete(r)}>
            <Trash2 size={16} aria-hidden /> Delete
          </button>
        </div>
      </div>
      {remove.isError && <ErrorBanner message={errorMessage(remove.error)} />}
      {pdf.isError && <ErrorBanner message={errorMessage(pdf.error)} />}
      {sharing && <ShareDialog ids={[r.id]} onClose={() => setSharing(false)} />}

      <header className="recipe-header">
        {r.photo_path && <img className="hero" src={mediaUrl(r.photo_path, r.updated_at)} alt="" />}
        <div>
          <h1>{r.title}</h1>
          {r.rating !== null && <Stars value={r.rating} />}
          {r.shared_by && (
            <p className="shared-by">
              <Link to="/friends" className="chip">
                From {r.shared_by}
              </Link>
            </p>
          )}
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
                <LabelChip key={c} name={c} labels={categories.data} param="c" />
              ))}
              {r.tags.map((t) => (
                <LabelChip key={t} name={t} labels={tags.data} param="t" />
              ))}
            </div>
          )}
        </div>
      </header>

      <RecipeVideo recipe={r} onChange={() => void recipe.refetch()} />

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
                  <span className="qty">{[l.quantity_text, l.unit].filter(Boolean).join(" ")}</span>{" "}
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

/** The downloaded video with standard controls, or the download status and actions. */
function RecipeVideo({ recipe: r, onChange }: { recipe: Recipe; onChange: () => void }) {
  const dl = useVideoDownload(r.id, onChange);
  const video = useRef<HTMLVideoElement>(null);
  const player: Player = {
    seek: (seconds) => {
      const v = video.current;
      if (!v) return;
      v.currentTime = seconds;
      void v.play();
    },
    position: () => video.current?.currentTime ?? 0,
    duration: () => {
      const d = video.current?.duration;
      return d && Number.isFinite(d) ? d : null;
    },
  };
  const playable = r.video_status === "ready" && r.video_path !== null && !dl.removed;
  if (!playable && !r.youtube_url && !isRunning(dl.job)) return null;
  return (
    <section className="recipe-video">
      {playable && (
        <video
          ref={video}
          controls
          preload="metadata"
          src={mediaUrl(r.video_path!, r.updated_at)}
          poster={r.photo_path ? mediaUrl(r.photo_path, r.updated_at) : undefined}
        />
      )}
      {playable && <VideoBookmarks recipeId={r.id} bookmarks={r.bookmarks} player={player} />}
      {(!playable || isRunning(dl.job) || dl.job?.stage === "failed") && (
        <VideoStatusPanel
          dl={dl}
          saved={{ status: r.video_status, url: r.youtube_url }}
          url={r.youtube_url ?? ""}
        />
      )}
    </section>
  );
}

const MAX_BOOKMARKS = 12;

/** What bookmarks need from the video element. */
type Player = {
  seek: (seconds: number) => void;
  position: () => number;
  /** Length in seconds, once known. */
  duration: () => number | null;
};

/** "m:ss" / "h:mm:ss", to pre-fill the time box with the player's position. */
function clock(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const pad = (n: number) => String(n).padStart(2, "0");
  const h = Math.floor(s / 3600);
  const m = Math.floor(s / 60) % 60;
  return h ? `${h}:${pad(m)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`;
}

/**
 * Bookmarks under the video, in time order (Rust sorts them): click one to jump there.
 * "Add bookmark" starts at the player's current position.
 */
function VideoBookmarks({
  recipeId,
  bookmarks,
  player,
}: {
  recipeId: string;
  bookmarks: VideoBookmark[];
  player: Player;
}) {
  const queryClient = useQueryClient();
  const [adding, setAdding] = useState(false);
  const [time, setTime] = useState("");
  const [label, setLabel] = useState("");

  const store = (list: VideoBookmark[]) =>
    queryClient.setQueryData<Recipe>(queryKeys.recipe(recipeId), (r) =>
      r ? { ...r, bookmarks: list } : r,
    );

  const add = useMutation({
    mutationFn: () => api.addVideoBookmark(recipeId, time, label, player.duration()),
    onSuccess: (list) => {
      store(list);
      setAdding(false);
    },
  });
  const remove = useMutation({
    mutationFn: (id: number) => api.deleteVideoBookmark(recipeId, id),
    onSuccess: store,
  });

  const startAdding = () => {
    setTime(clock(player.position()));
    setLabel("");
    add.reset();
    setAdding(true);
  };

  const full = bookmarks.length >= MAX_BOOKMARKS;
  return (
    <div className="bookmarks">
      <div className="bookmark-list">
        {bookmarks.map((b) => (
          <span key={b.id} className="bookmark">
            <button
              type="button"
              className="bookmark-jump"
              onClick={() => player.seek(b.seconds)}
              title={`Jump to ${b.time_text}`}
            >
              <span className="bookmark-time">{b.time_text}</span>
              {b.label && <span className="bookmark-label">{b.label}</span>}
            </button>
            <button
              type="button"
              className="bookmark-remove"
              aria-label={`Remove bookmark at ${b.time_text}`}
              title="Remove bookmark"
              onClick={() => remove.mutate(b.id)}
            >
              <X size={12} aria-hidden />
            </button>
          </span>
        ))}
        {!adding && (
          <button
            type="button"
            className="button small"
            disabled={full}
            onClick={startAdding}
            title={
              full ? `${MAX_BOOKMARKS} bookmarks is the limit` : "Bookmark a point in the video"
            }
          >
            <BookmarkPlus size={14} aria-hidden /> Add bookmark
          </button>
        )}
      </div>
      {adding && (
        <form
          className="bookmark-form"
          onSubmit={(e) => {
            e.preventDefault();
            add.mutate();
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape") setAdding(false);
          }}
        >
          <label>
            Time
            <input
              value={time}
              onChange={(e) => setTime(e.target.value)}
              placeholder="2:15"
              size={8}
              autoFocus
              onFocus={(e) => e.target.select()}
            />
          </label>
          <label className="grow">
            Name (optional)
            <input
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder="e.g. Making the sauce"
              maxLength={40}
            />
          </label>
          <button type="submit" className="button primary small" disabled={add.isPending}>
            Save bookmark
          </button>
          <button type="button" className="button ghost small" onClick={() => setAdding(false)}>
            Cancel
          </button>
        </form>
      )}
      {full && !adding && (
        <p className="hint">
          That’s the limit of {MAX_BOOKMARKS} bookmarks; remove one to add another.
        </p>
      )}
      {add.isError && <ErrorBanner message={errorMessage(add.error)} />}
      {remove.isError && <ErrorBanner message={errorMessage(remove.error)} />}
    </div>
  );
}

/** A category or tag chip that opens the library filtered by it. */
function LabelChip({
  name,
  labels,
  param,
}: {
  name: string;
  labels: Label[] | undefined;
  param: "c" | "t";
}) {
  const className = param === "t" ? "chip tag" : "chip";
  const text = param === "t" ? `#${name}` : name;
  const label = labels?.find((l) => l.name === name);
  if (!label) return <span className={className}>{text}</span>;
  return (
    <Link
      to={`/?${param}=${label.id}`}
      className={`${className} link-chip`}
      title={`Show all “${name}” recipes`}
    >
      {text}
    </Link>
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
