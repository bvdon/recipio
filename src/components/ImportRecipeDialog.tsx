import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { useMutation } from "@tanstack/react-query";
import { Globe, LoaderCircle } from "lucide-react";
import { api, errorMessage } from "../api";
import type { WebImport } from "../bindings/WebImport";
import { ErrorBanner } from "./common";

/** Router state handed to the new-recipe editor after an import. */
export type ImportState = { webImport: WebImport };

/**
 * Asks for a recipe page's address; Rust reads the page and the result opens in the
 * new-recipe editor for review. Nothing is saved here.
 */
export default function ImportRecipeDialog({ onClose }: { onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const navigate = useNavigate();
  const [url, setUrl] = useState("");
  const [noRecipe, setNoRecipe] = useState<string | null>(null);
  const run = useMutation({ mutationFn: api.importRecipe });

  useEffect(() => {
    dialog.current?.showModal();
  }, []);

  const open = (webImport: WebImport) => {
    onClose();
    navigate("/recipes/new", { state: { webImport } satisfies ImportState });
  };

  const start = (text: string) => {
    if (text.trim() === "" || run.isPending) return;
    setNoRecipe(null);
    // Per-call callbacks: they don't fire if the dialog was closed in the meantime.
    run.mutate(text, {
      onSuccess: (result) =>
        result.outcome === "found" ? open(result) : setNoRecipe(result.source_url),
    });
  };

  return (
    <dialog
      ref={dialog}
      className="import-dialog"
      aria-labelledby="import-title"
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
    >
      <h2 id="import-title">Import a recipe from the web</h2>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          start(url);
        }}
      >
        <label>
          Recipe page address
          <input
            value={url}
            onChange={(e) => {
              setUrl(e.target.value);
              setNoRecipe(null);
              run.reset();
            }}
            onPaste={(e) => {
              // Pasting into the empty field starts right away.
              const text = e.clipboardData.getData("text/plain").trim();
              if (url.trim() === "" && text) {
                e.preventDefault();
                setUrl(text);
                start(text);
              }
            }}
            placeholder="https://…"
            type="url"
            disabled={run.isPending}
            autoFocus
            spellCheck={false}
          />
        </label>
        <p className="hint">
          Works with most recipe sites. Everything opens in the editor for you to check before it’s
          saved.
        </p>

        {run.isError && <ErrorBanner message={errorMessage(run.error)} />}
        {noRecipe && (
          <div className="import-none" role="status">
            <p>
              This page doesn’t include recipe information Recipio can read. You can still create
              the recipe by hand and keep the page as its source.
            </p>
            <button
              type="button"
              className="button"
              onClick={() => open({ outcome: "no_recipe", source_url: noRecipe })}
            >
              Create recipe with just the link
            </button>
          </div>
        )}

        <div className="actions dialog-actions">
          <button type="button" className="button" onClick={onClose}>
            Cancel
          </button>
          <button
            type="submit"
            className="button primary"
            disabled={run.isPending || url.trim() === ""}
          >
            {run.isPending ? (
              <>
                <LoaderCircle size={16} aria-hidden className="spin" /> Reading page…
              </>
            ) : (
              <>
                <Globe size={16} aria-hidden /> Import
              </>
            )}
          </button>
        </div>
      </form>
    </dialog>
  );
}
