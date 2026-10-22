import { type KeyboardEvent, type Ref, useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Heading, Plus, X } from "lucide-react";
import { api } from "../api";
import SortableList from "./SortableList";

export type IngredientRow = {
  key: string;
  is_heading: boolean;
  /** Raw quantity text as typed, e.g. "1 1/2" or "2-3". Parsed by Rust on save. */
  qty: string;
  unit: string;
  item: string;
  prep_note: string;
};

export function blankRow(is_heading = false): IngredientRow {
  return { key: crypto.randomUUID(), is_heading, qty: "", unit: "", item: "", prep_note: "" };
}

type Props = {
  rows: IngredientRow[];
  onChange: (rows: IngredientRow[]) => void;
};

/**
 * Keyboard-first ingredient entry: Enter adds a line below, Backspace in an empty line
 * removes it, drag (or keyboard via the handle) to reorder.
 */
export default function IngredientsEditor({ rows, onChange }: Props) {
  const inputs = useRef(new Map<string, HTMLInputElement>());
  // Row to focus after the next render (set when a row is added or removed).
  const pendingFocus = useRef<string | null>(null);

  const focusAfterRender = (key: string) => {
    pendingFocus.current = key;
  };

  useEffect(() => {
    if (pendingFocus.current) {
      inputs.current.get(pendingFocus.current)?.focus();
      pendingFocus.current = null;
    }
  });

  const update = (key: string, patch: Partial<IngredientRow>) =>
    onChange(rows.map((r) => (r.key === key ? { ...r, ...patch } : r)));

  const insertAfter = (index: number, is_heading = false) => {
    const row = blankRow(is_heading);
    onChange([...rows.slice(0, index + 1), row, ...rows.slice(index + 1)]);
    focusAfterRender(row.key);
  };

  const remove = (index: number) => {
    onChange(rows.filter((_, i) => i !== index));
    const prev = rows[index - 1];
    if (prev) focusAfterRender(prev.key);
  };

  const onKeyDown = (e: KeyboardEvent, index: number) => {
    if (e.nativeEvent.isComposing) return;
    const row = rows[index];
    if (e.key === "Enter" && !e.shiftKey && !e.metaKey && !e.ctrlKey) {
      e.preventDefault();
      insertAfter(index);
    } else if (
      e.key === "Backspace" &&
      !row.qty &&
      !row.unit &&
      !row.item &&
      !row.prep_note &&
      rows.length > 1
    ) {
      e.preventDefault();
      remove(index);
    }
  };

  // The first input of each row receives focus when the row is created.
  const firstInputRef = (key: string) => (el: HTMLInputElement | null) => {
    if (el) inputs.current.set(key, el);
    else inputs.current.delete(key);
  };

  return (
    <div className="ingredients-editor">
      {rows.length > 0 && (
        <div className="ingredient-columns" aria-hidden>
          <span />
          <span>Qty</span>
          <span>Unit</span>
          <span>Ingredient</span>
          <span>Prep note</span>
          <span />
        </div>
      )}
      <SortableList
        items={rows}
        onReorder={onChange}
        className="ingredient-rows"
        renderItem={(row, index, handle) =>
          row.is_heading ? (
            <div className="ingredient-row heading" onKeyDown={(e) => onKeyDown(e, index)}>
              {handle}
              <input
                ref={firstInputRef(row.key)}
                value={row.item}
                onChange={(e) => update(row.key, { item: e.target.value })}
                placeholder="Section heading, e.g. For the sauce"
                aria-label="Section heading"
              />
              <RemoveButton onClick={() => remove(index)} />
            </div>
          ) : (
            <div className="ingredient-row" onKeyDown={(e) => onKeyDown(e, index)}>
              {handle}
              <QuantityInput
                ref={firstInputRef(row.key)}
                value={row.qty}
                onChange={(qty) => update(row.key, { qty })}
              />
              <input
                value={row.unit}
                onChange={(e) => update(row.key, { unit: e.target.value })}
                placeholder="cups"
                aria-label="Unit"
              />
              <input
                value={row.item}
                onChange={(e) => update(row.key, { item: e.target.value })}
                placeholder="flour"
                aria-label="Ingredient"
              />
              <input
                value={row.prep_note}
                onChange={(e) => update(row.key, { prep_note: e.target.value })}
                placeholder="sifted"
                aria-label="Prep note"
              />
              <RemoveButton onClick={() => remove(index)} />
            </div>
          )
        }
      />
      <div className="row-actions">
        <button type="button" className="button" onClick={() => insertAfter(rows.length - 1)}>
          <Plus size={16} aria-hidden /> Ingredient
        </button>
        <button type="button" className="button" onClick={() => insertAfter(rows.length - 1, true)}>
          <Heading size={16} aria-hidden /> Section heading
        </button>
      </div>
      <p className="hint">
        Enter adds a line · Backspace on an empty line removes it · Quantities like 1/2, 1 1/2, ¾ or
        2-3 are fine
      </p>
    </div>
  );
}

/** Quantity field, checked by the Rust parser; marked invalid once you leave it. */
function QuantityInput({
  ref,
  value,
  onChange,
}: {
  ref: Ref<HTMLInputElement>;
  value: string;
  onChange: (value: string) => void;
}) {
  const [focused, setFocused] = useState(false);
  const text = value.trim();
  const check = useQuery({
    queryKey: ["check-quantity", text],
    queryFn: () => api.checkQuantity(text),
    enabled: text !== "",
    staleTime: Infinity,
  });
  const invalid = !focused && text !== "" && check.data === false;
  return (
    <input
      ref={ref}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      onFocus={() => setFocused(true)}
      onBlur={() => setFocused(false)}
      className={invalid ? "invalid" : ""}
      title={invalid ? "Not a quantity — try 2, 1.5, 1/2, 1 1/2 or 2-3" : undefined}
      aria-invalid={invalid}
      placeholder="1/2"
      aria-label="Quantity"
    />
  );
}

export function RemoveButton({
  onClick,
  label = "Remove",
}: {
  onClick: () => void;
  label?: string;
}) {
  return (
    <button
      type="button"
      className="icon-button"
      onClick={onClick}
      aria-label={label}
      title={label}
    >
      <X size={16} aria-hidden />
    </button>
  );
}
