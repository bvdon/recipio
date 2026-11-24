import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { useMutation } from "@tanstack/react-query";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as chooseFile } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink, FileDown, Globe, LoaderCircle } from "lucide-react";
import { api, errorMessage, isImportBlocked } from "../api";
import type { WebImport } from "../bindings/WebImport";
import { ErrorBanner } from "./common";

/** Router state handed to the new-recipe editor after an import. */
export type ImportState = { webImport: WebImport };

/** What to import: a page for Rust to fetch, or a copy the user saved from their browser. */
type ImportSource = { url: string; savedPage?: string };

/**
 * Asks for a recipe page's address; Rust reads the page and the result opens in the
 * new-recipe editor for review. Nothing is saved here. For sites that refuse apps, a page
 * saved in the browser can be chosen or dropped anywhere on the window instead.
 */
export default function ImportRecipeDialog({ onClose }: { onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const navigate = useNavigate();
  const [url, setUrl] = useState("");
  const [noRecipe, setNoRecipe] = useState<string | null>(null); // the page's address, may be ""
  const run = useMutation({
    mutationFn: ({ url, savedPage }: ImportSource) =>
      savedPage ? api.importSavedPage(savedPage, url) : api.importRecipe(url),
  });
  const [dropping, setDropping] = useState(false);
  // Kept apart from the latest error, so choosing a wrong file doesn't hide the instructions.
  const [blocked, setBlocked] = useState(false);

  useEffect(() => {
    dialog.current?.showModal();
  }, []);

  const open = (webImport: WebImport) => {
    onClose();
    navigate("/recipes/new", { state: { webImport } satisfies ImportState });
  };

  const start = (source: ImportSource) => {
    if (run.isPending || (!source.savedPage && source.url.trim() === "")) return;
    setNoRecipe(null);
    // Per-call callbacks: they don't fire if the dialog was closed in the meantime.
    run.mutate(source, {
      onError: (e) => {
        if (isImportBlocked(e)) setBlocked(true);
      },
      onSuccess: (result) =>
        result.outcome === "found" ? open(result) : setNoRecipe(result.source_url),
    });
  };

  const startSaved = (savedPage: string) => start({ url, savedPage });

  const chooseSaved = async () => {
    const chosen = await chooseFile({
      multiple: false,
      directory: false,
      // Formats Recipio can't read are selectable too, so Rust can explain what to pick
      // instead of the file just being greyed out.
      filters: [
        { name: "Saved web page", extensions: ["html", "htm", "xhtml", "webarchive", "mhtml"] },
      ],
    });
    if (chosen) startSaved(chosen);
  };

  // Tauri reports dropped files for the whole window, and only its event has their paths.
  // The listener is added once; the ref gives it the current handler.
  const onDropFile = useRef(startSaved);
  useEffect(() => {
    onDropFile.current = startSaved;
  });
  useEffect(() => {
    const unlisten = getCurrentWebview().onDragDropEvent(({ payload }) => {
      if (payload.type === "enter") setDropping(payload.paths.length > 0);
      else if (payload.type === "leave") setDropping(false);
      else if (payload.type === "drop") {
        setDropping(false);
        if (payload.paths[0]) onDropFile.current(payload.paths[0]);
      }
    });
    return () => void unlisten.then((stop) => stop());
  }, []);

  const openInBrowser = () => {
    const address = url.trim();
    // Rust accepts an address without a scheme; the browser needs one.
    void openUrl(/^https?:\/\//i.test(address) ? address : `https://${address}`);
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
          start({ url });
        }}
      >
        <label>
          Recipe page address
          <input
            value={url}
            onChange={(e) => {
              setUrl(e.target.value);
              setNoRecipe(null);
              setBlocked(false);
              run.reset();
            }}
            onPaste={(e) => {
              // Pasting into the empty field starts right away.
              const text = e.clipboardData.getData("text/plain").trim();
              if (url.trim() === "" && text) {
                e.preventDefault();
                setUrl(text);
                start({ url: text });
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
        {noRecipe !== null && (
          <div className="import-none" role="status">
            <p>
              This page doesn’t include recipe information Recipio can read. You can still create
              the recipe by hand{noRecipe !== "" && " and keep the page as its source"}.
            </p>
            {noRecipe !== "" && (
              <button
                type="button"
                className="button"
                onClick={() => open({ outcome: "no_recipe", source_url: noRecipe })}
              >
                Create recipe with just the link
              </button>
            )}
          </div>
        )}

        <div className={dropping ? "import-saved dropping" : "import-saved"}>
          {blocked ? (
            <>
              <p>You can still import this recipe from a copy saved in your browser:</p>
              <ol>
                <li>Open the page in your browser and wait for the recipe to appear.</li>
                <li>
                  Save it with Ctrl+S (⌘S on a Mac). As the format, choose{" "}
                  <strong>Page Source</strong> in Safari, or the <strong>HTML Only</strong> option
                  in other browsers.
                </li>
                <li>Choose the saved file here, or drop it on this window.</li>
              </ol>
            </>
          ) : (
            <p>Site won’t let Recipio in? Choose or drop a page saved from your browser.</p>
          )}
          <div className="row-actions">
            {blocked && (
              <button type="button" className="button" onClick={openInBrowser}>
                <ExternalLink size={16} aria-hidden /> Open in browser
              </button>
            )}
            <button
              type="button"
              className="button"
              disabled={run.isPending}
              onClick={() => void chooseSaved()}
            >
              <FileDown size={16} aria-hidden /> Choose saved page…
            </button>
          </div>
        </div>

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
