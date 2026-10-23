import { useEffect, useRef, useState } from "react";
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { api } from "../api";
import type { ParsedIngredient } from "../bindings/ParsedIngredient";

type Props = {
  /** Text to start with, e.g. a multi-line paste into an ingredient field. */
  initialText: string;
  onApply: (lines: ParsedIngredient[]) => void;
  onClose: () => void;
};

/**
 * Paste a block of ingredient text; Rust splits it into lines, shown here for review
 * before they are added to the editor (where they can still be changed).
 */
export default function PasteIngredientsDialog({ initialText, onApply, onClose }: Props) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [text, setText] = useState(initialText);

  useEffect(() => {
    dialog.current?.showModal();
  }, []);

  const parsed = useQuery({
    queryKey: ["parse-ingredients", text],
    queryFn: () => api.parseIngredients(text),
    placeholderData: keepPreviousData,
    staleTime: Infinity,
  });
  const lines = text.trim() ? (parsed.data ?? []) : [];
  const count = lines.filter((l) => !l.is_heading).length;

  return (
    <dialog
      ref={dialog}
      className="paste-dialog"
      aria-labelledby="paste-title"
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
    >
      <h2 id="paste-title">Paste ingredients</h2>
      <div className="paste-body">
        <label className="paste-input">
          <span className="hint">
            One ingredient per line. Lines ending in “:” become headings.
          </span>
          <textarea
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder={"For the dough:\n2 1/2 cups flour, sifted\n1 tsp salt\n2 large eggs"}
            autoFocus
            spellCheck={false}
          />
        </label>
        <div className="paste-preview" aria-live="polite">
          {lines.length === 0 ? (
            <p className="hint">The parsed lines will appear here for you to check.</p>
          ) : (
            <table>
              <thead>
                <tr>
                  <th>Qty</th>
                  <th>Unit</th>
                  <th>Ingredient</th>
                  <th>Prep note</th>
                </tr>
              </thead>
              <tbody>
                {lines.map((l, i) =>
                  l.is_heading ? (
                    <tr key={i} className="heading">
                      <td colSpan={4}>{l.item}</td>
                    </tr>
                  ) : (
                    <tr key={i}>
                      <td>{l.quantity}</td>
                      <td>{l.unit}</td>
                      <td>{l.item}</td>
                      <td>{l.prep_note}</td>
                    </tr>
                  ),
                )}
              </tbody>
            </table>
          )}
        </div>
      </div>
      <div className="actions paste-actions">
        <span className="hint">You can still edit every line after adding.</span>
        <button type="button" className="button" onClick={onClose}>
          Cancel
        </button>
        <button
          type="button"
          className="button primary"
          disabled={lines.length === 0}
          onClick={() => onApply(lines)}
        >
          Add {count} {count === 1 ? "ingredient" : "ingredients"}
        </button>
      </div>
    </dialog>
  );
}
