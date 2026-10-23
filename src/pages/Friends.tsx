import { useState } from "react";
import { Link } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ask } from "@tauri-apps/plugin-dialog";
import {
  ArrowDownLeft,
  ArrowUpRight,
  BookOpen,
  FileDown,
  Pencil,
  Share2,
  Trash2,
  UserPlus,
} from "lucide-react";
import { api, errorMessage, queryKeys } from "../api";
import type { Friend } from "../bindings/Friend";
import type { ShareLogEntry } from "../bindings/ShareLogEntry";
import type { ShareMethod } from "../bindings/ShareMethod";
import { ErrorBanner, plural } from "../components/common";
import MyName from "../components/MyName";

/** Friends you send recipes to, how to reach them, and what you've shared. */
export default function Friends() {
  const friends = useQuery({ queryKey: queryKeys.friends, queryFn: api.listFriends });
  const history = useQuery({
    queryKey: queryKeys.history(null),
    queryFn: () => api.shareHistory(null),
  });
  const [editing, setEditing] = useState<Friend | "new" | null>(null);

  return (
    <div className="friends-page">
      <div className="toolbar">
        <h1>Friends</h1>
        <div className="actions">
          <Link to="/share" className="button">
            <Share2 size={16} aria-hidden /> Share recipes…
          </Link>
          <Link to="/share?all=1" className="button">
            <BookOpen size={16} aria-hidden /> Share the whole book…
          </Link>
          <Link to="/share/import" className="button">
            <FileDown size={16} aria-hidden /> Import a file…
          </Link>
        </div>
      </div>
      <p className="muted">
        Send recipes with the apps you already use: on a Mac, Messages or Mail opens with the
        recipes attached. Friends who use Recipio click the attachment to add them. Everything here
        stays on your computer.
      </p>

      <MyName />

      <section>
        <div className="section-head">
          <h2>Your friends</h2>
          {editing !== "new" && (
            <button type="button" className="button primary" onClick={() => setEditing("new")}>
              <UserPlus size={16} aria-hidden /> Add a friend
            </button>
          )}
        </div>
        {editing === "new" && <FriendForm onDone={() => setEditing(null)} />}
        {friends.isError && <ErrorBanner message={errorMessage(friends.error)} />}
        {friends.data?.length === 0 && editing !== "new" && (
          <p className="muted">No friends yet. Add someone to send them recipes in one click.</p>
        )}
        <ul className="friend-list">
          {friends.data?.map((f) =>
            editing !== "new" && editing?.id === f.id ? (
              <li key={f.id}>
                <FriendForm friend={f} onDone={() => setEditing(null)} />
              </li>
            ) : (
              <FriendRow
                key={f.id}
                friend={f}
                log={(history.data ?? []).filter((e) => e.friend_id === f.id)}
                onEdit={() => setEditing(f)}
              />
            ),
          )}
        </ul>
      </section>

      <section>
        <h2>Recent activity</h2>
        {history.data?.length === 0 && <p className="muted">Nothing shared yet.</p>}
        <ul className="history">
          {history.data?.slice(0, 30).map((e) => (
            <HistoryLine key={e.id} entry={e} showPerson />
          ))}
        </ul>
      </section>
    </div>
  );
}

function FriendRow({
  friend: f,
  log,
  onEdit,
}: {
  friend: Friend;
  log: ShareLogEntry[];
  onEdit: () => void;
}) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const remove = useMutation({
    mutationFn: () => api.deleteFriend(f.id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.friends });
      void queryClient.invalidateQueries({ queryKey: queryKeys.allHistory });
    },
  });
  const confirmRemove = async () => {
    const ok = await ask(`Remove ${f.name} from your friends? What you've shared is kept.`, {
      title: "Remove friend",
      kind: "warning",
      okLabel: "Remove",
      cancelLabel: "Cancel",
    });
    if (ok) remove.mutate();
  };
  const initials = f.name
    .split(/\s+/)
    .map((w) => w[0])
    .join("")
    .slice(0, 2)
    .toUpperCase();
  return (
    <li className="friend">
      <div className="friend-main">
        <span className="avatar" aria-hidden>
          {initials}
        </span>
        <div className="friend-info">
          <strong>{f.name}</strong>
          <span className="muted">
            {f.method === "messages" ? "Messages" : "Email"} · {f.address}
          </span>
          {log.length > 0 && (
            <button type="button" className="link small" onClick={() => setOpen((o) => !o)}>
              {open ? "Hide history" : `${plural(log.length, "share")} — show history`}
            </button>
          )}
        </div>
        <div className="row-actions">
          <Link to={`/share?friend=${f.id}`} className="button small">
            <Share2 size={14} aria-hidden /> Share recipes…
          </Link>
          <button type="button" className="button small ghost" onClick={onEdit}>
            <Pencil size={14} aria-hidden /> Edit
          </button>
          <button
            type="button"
            className="button small ghost"
            onClick={() => void confirmRemove()}
            aria-label={`Remove ${f.name}`}
          >
            <Trash2 size={14} aria-hidden />
          </button>
        </div>
      </div>
      {open && (
        <ul className="history">
          {log.map((e) => (
            <HistoryLine key={e.id} entry={e} />
          ))}
        </ul>
      )}
      {remove.isError && <ErrorBanner message={errorMessage(remove.error)} />}
    </li>
  );
}

function HistoryLine({
  entry: e,
  showPerson = false,
}: {
  entry: ShareLogEntry;
  showPerson?: boolean;
}) {
  const when = new Date(e.at).toLocaleDateString(undefined, { dateStyle: "medium" });
  const sent = e.direction === "sent";
  const who = e.person || (sent ? "another app" : "someone");
  const titles =
    e.titles.length > 3 ? `${e.titles.slice(0, 3).join(", ")}, …` : e.titles.join(", ");
  return (
    <li>
      {sent ? (
        <ArrowUpRight size={14} aria-label="Sent" className="sent" />
      ) : (
        <ArrowDownLeft size={14} aria-label="Received" className="received" />
      )}
      <span>
        {sent ? "Sent" : "Received"} {plural(e.titles.length, "recipe")}
        {showPerson && (sent ? ` to ${who}` : ` from ${who}`)}
        {titles && <span className="muted"> — {titles}</span>}
      </span>
      <span className="muted when">{when}</span>
    </li>
  );
}

function FriendForm({ friend, onDone }: { friend?: Friend; onDone: () => void }) {
  const queryClient = useQueryClient();
  const [name, setName] = useState(friend?.name ?? "");
  const [method, setMethod] = useState<ShareMethod>(friend?.method ?? "messages");
  const [address, setAddress] = useState(friend?.address ?? "");
  const save = useMutation({
    mutationFn: () => {
      const input = { name, method, address };
      return friend ? api.updateFriend(friend.id, input) : api.addFriend(input);
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.friends });
      onDone();
    },
  });
  return (
    <form
      className="friend-form"
      onSubmit={(e) => {
        e.preventDefault();
        save.mutate();
      }}
      onKeyDown={(e) => {
        if (e.key === "Escape") onDone();
      }}
    >
      <label>
        Name
        <input value={name} onChange={(e) => setName(e.target.value)} autoFocus required />
      </label>
      <label>
        Send with
        <select value={method} onChange={(e) => setMethod(e.target.value as ShareMethod)}>
          <option value="messages">Messages (iPhone, iPad, Mac)</option>
          <option value="email">Email</option>
        </select>
      </label>
      <label className="grow">
        {method === "messages" ? "Phone number or Apple ID email" : "Email address"}
        <input
          value={address}
          onChange={(e) => setAddress(e.target.value)}
          placeholder={method === "messages" ? "+1 555 123 4567" : "sam@example.com"}
          required
        />
      </label>
      <div className="row-actions">
        <button type="submit" className="button primary" disabled={save.isPending}>
          {friend ? "Save" : "Add friend"}
        </button>
        <button type="button" className="button ghost" onClick={onDone}>
          Cancel
        </button>
      </div>
      {method === "messages" && (
        <p className="hint full">
          Messages only reaches Apple devices. For friends on Windows, choose Email.
        </p>
      )}
      {save.isError && <ErrorBanner message={errorMessage(save.error)} />}
    </form>
  );
}
