import { useState } from "react";
import { Link } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ask } from "@tauri-apps/plugin-dialog";
import { ArrowLeft, Check, Merge, Pencil, Plus, Trash2, X } from "lucide-react";
import { api, errorMessage, queryKeys } from "../api";
import type { Label } from "../bindings/Label";
import type { LabelKind } from "../bindings/LabelKind";
import { ErrorBanner } from "../components/common";

export default function Organize() {
  return (
    <section>
      <div className="toolbar">
        <Link to="/" className="button ghost">
          <ArrowLeft size={16} aria-hidden /> Recipes
        </Link>
      </div>
      <h1>Categories &amp; tags</h1>
      <p className="muted">
        Rename, delete or merge. Changes apply to every recipe; deleting never deletes recipes.
      </p>
      <div className="organize-grid">
        <LabelManager kind="category" title="Categories" singular="category" />
        <LabelManager kind="tag" title="Tags" singular="tag" />
      </div>
    </section>
  );
}

const recipesText = (n: number) => `${n} recipe${n === 1 ? "" : "s"}`;

function LabelManager({
  kind,
  title,
  singular,
}: {
  kind: LabelKind;
  title: string;
  singular: string;
}) {
  const queryClient = useQueryClient();
  const labels = useQuery({
    queryKey: queryKeys.labels(kind),
    queryFn: () => api.listLabels(kind),
  });
  const [newName, setNewName] = useState("");
  const [editing, setEditing] = useState<{ id: number; name: string } | null>(null);
  const [selected, setSelected] = useState<number[]>([]);
  const [mergeTarget, setMergeTarget] = useState<number | null>(null);

  // Any change can alter names shown on recipes, counts, and library filters.
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: queryKeys.allLabels });
    void queryClient.invalidateQueries({ queryKey: queryKeys.recipes });
    void queryClient.invalidateQueries({ queryKey: queryKeys.allRecipes });
  };

  const create = useMutation({
    mutationFn: (name: string) => api.createLabel(kind, name),
    onSuccess: () => {
      setNewName("");
      refresh();
    },
  });
  const rename = useMutation({
    mutationFn: ({ id, name }: { id: number; name: string }) => api.renameLabel(kind, id, name),
    onSuccess: () => {
      setEditing(null);
      refresh();
    },
  });
  const remove = useMutation({
    mutationFn: (id: number) => api.deleteLabel(kind, id),
    onSuccess: (_, id) => {
      setSelected((s) => s.filter((x) => x !== id));
      refresh();
    },
  });
  const merge = useMutation({
    mutationFn: ({ sources, target }: { sources: number[]; target: number }) =>
      api.mergeLabels(kind, sources, target),
    onSuccess: () => {
      setSelected([]);
      setMergeTarget(null);
      refresh();
    },
  });
  const failed = [create, rename, remove, merge].find((m) => m.isError);

  const all = labels.data ?? [];
  const byId = (id: number) => all.find((l) => l.id === id);
  const selectedLabels = selected.map(byId).filter((l): l is Label => l !== undefined);
  const target = selectedLabels.find((l) => l.id === mergeTarget) ?? selectedLabels[0];

  const toggleSelected = (id: number) =>
    setSelected((s) => (s.includes(id) ? s.filter((x) => x !== id) : [...s, id]));

  const confirmDelete = async (l: Label) => {
    const ok = await ask(
      `Delete the ${singular} “${l.name}”?${l.recipe_count > 0 ? ` It will be removed from ${recipesText(l.recipe_count)}; the recipes are kept.` : ""}`,
      { title: `Delete ${singular}`, kind: "warning", okLabel: "Delete", cancelLabel: "Cancel" },
    );
    if (ok) remove.mutate(l.id);
  };

  const confirmMerge = async () => {
    if (!target) return;
    const sources = selectedLabels.filter((l) => l.id !== target.id);
    const ok = await ask(
      `Merge ${sources.map((l) => `“${l.name}”`).join(", ")} into “${target.name}”? Recipes using any of them will use “${target.name}” instead.`,
      {
        title: `Merge ${title.toLowerCase()}`,
        kind: "warning",
        okLabel: "Merge",
        cancelLabel: "Cancel",
      },
    );
    if (ok) merge.mutate({ sources: sources.map((l) => l.id), target: target.id });
  };

  const commitRename = () => {
    if (!editing) return;
    const original = byId(editing.id)?.name;
    if (editing.name.trim() === "" || editing.name === original) setEditing(null);
    else rename.mutate(editing);
  };

  return (
    <div className="label-manager">
      <h2>{title}</h2>
      {failed && <ErrorBanner message={errorMessage(failed.error)} />}

      <form
        className="add-label"
        onSubmit={(e) => {
          e.preventDefault();
          if (newName.trim()) create.mutate(newName);
        }}
      >
        <input
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          placeholder={`New ${singular}`}
          aria-label={`New ${singular} name`}
        />
        <button type="submit" className="button" disabled={!newName.trim() || create.isPending}>
          <Plus size={16} aria-hidden /> Add
        </button>
      </form>

      {selectedLabels.length >= 2 && target && (
        <div className="merge-bar">
          <Merge size={16} aria-hidden />
          <span>Merge {selectedLabels.length} into</span>
          <select
            value={target.id}
            onChange={(e) => setMergeTarget(Number(e.target.value))}
            aria-label="Merge into"
          >
            {selectedLabels.map((l) => (
              <option key={l.id} value={l.id}>
                {l.name}
              </option>
            ))}
          </select>
          <button
            className="button primary"
            onClick={() => void confirmMerge()}
            disabled={merge.isPending}
          >
            Merge
          </button>
          <button className="button ghost" onClick={() => setSelected([])}>
            Cancel
          </button>
        </div>
      )}

      {labels.data && all.length === 0 && (
        <p className="muted">
          No {title.toLowerCase()} yet. Add them here or while editing a recipe.
        </p>
      )}
      {all.length > 0 && <p className="hint">Select two or more to merge them.</p>}
      <ul className="label-rows">
        {all.map((l) => (
          <li key={l.id} className={selected.includes(l.id) ? "selected" : ""}>
            <input
              type="checkbox"
              checked={selected.includes(l.id)}
              onChange={() => toggleSelected(l.id)}
              aria-label={`Select ${l.name}`}
            />
            {editing?.id === l.id ? (
              <>
                <input
                  className="rename-input"
                  value={editing.name}
                  autoFocus
                  aria-label={`Rename ${l.name}`}
                  onChange={(e) => setEditing({ id: l.id, name: e.target.value })}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") commitRename();
                    if (e.key === "Escape") setEditing(null);
                  }}
                />
                <button
                  className="icon-button neutral"
                  onClick={commitRename}
                  aria-label="Save name"
                  title="Save (Enter)"
                >
                  <Check size={16} aria-hidden />
                </button>
                <button
                  className="icon-button neutral"
                  onClick={() => setEditing(null)}
                  aria-label="Cancel rename"
                  title="Cancel (Esc)"
                >
                  <X size={16} aria-hidden />
                </button>
              </>
            ) : (
              <>
                <span className="name">{l.name}</span>
                <span className="muted small">{recipesText(l.recipe_count)}</span>
                <button
                  className="icon-button neutral"
                  onClick={() => setEditing({ id: l.id, name: l.name })}
                  aria-label={`Rename ${l.name}`}
                  title="Rename"
                >
                  <Pencil size={16} aria-hidden />
                </button>
                <button
                  className="icon-button"
                  onClick={() => void confirmDelete(l)}
                  aria-label={`Delete ${l.name}`}
                  title="Delete"
                >
                  <Trash2 size={16} aria-hidden />
                </button>
              </>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
