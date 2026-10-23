import { useCallback, useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { error as logError } from "@tauri-apps/plugin-log";
import { Check, ChevronLeft, ChevronRight, ListChecks, X } from "lucide-react";
import { api, errorMessage, queryKeys } from "../api";
import type { IngredientLine } from "../bindings/IngredientLine";
import { ErrorBanner } from "../components/common";

/**
 * Full-screen cooking view: one step at a time, ingredients in a side panel, screen kept
 * awake (Rust) while open.
 */
export default function CookingMode() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const recipe = useQuery({ queryKey: queryKeys.recipe(id), queryFn: () => api.getRecipe(id) });
  const [step, setStep] = useState(0);
  const [panel, setPanel] = useState(true);
  const [checked, setChecked] = useState<ReadonlySet<number>>(new Set());

  const count = recipe.data?.steps.length ?? 0;
  const last = Math.max(count - 1, 0);
  const next = useCallback(() => setStep((s) => Math.min(s + 1, last)), [last]);
  const prev = useCallback(() => setStep((s) => Math.max(s - 1, 0)), []);

  const exit = useCallback(() => {
    // Back to where cooking mode was opened from (normally the recipe page).
    const idx = (window.history.state as { idx?: number } | null)?.idx ?? 0;
    if (idx > 0) navigate(-1);
    else navigate(`/recipes/${id}`, { replace: true });
  }, [navigate, id]);

  // Full screen and screen kept awake while open; both undone on the way out.
  useEffect(() => {
    const win = getCurrentWindow();
    let left = false;
    let wasFullscreen = false;
    void win
      .isFullscreen()
      .then((full) => {
        wasFullscreen = full;
        if (!left && !full) return win.setFullscreen(true);
      })
      .catch((e: unknown) => void logError(`cooking mode: full screen failed: ${String(e)}`));
    void api.setKeepAwake(true);
    return () => {
      left = true;
      if (!wasFullscreen) void win.setFullscreen(false).catch(() => {});
      void api.setKeepAwake(false);
    };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const actions: Record<string, () => void> = {
        ArrowRight: next,
        ArrowDown: next,
        PageDown: next,
        ArrowLeft: prev,
        ArrowUp: prev,
        PageUp: prev,
        Home: () => setStep(0),
        End: () => setStep(last),
        Escape: exit,
        i: () => setPanel((p) => !p),
        I: () => setPanel((p) => !p),
      };
      const action = actions[e.key];
      if (action) {
        e.preventDefault();
        action();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [next, prev, exit, last]);

  if (recipe.isError) {
    return (
      <div className="cook">
        <ErrorBanner message={errorMessage(recipe.error)} />
        <button type="button" className="button" onClick={exit}>
          Back
        </button>
      </div>
    );
  }
  if (!recipe.data) return <div className="cook" />;
  const r = recipe.data;

  const toggle = (i: number) =>
    setChecked((c) => {
      const s = new Set(c);
      if (!s.delete(i)) s.add(i);
      return s;
    });

  return (
    <div className="cook">
      <header className="cook-bar">
        <h1 className="cook-title">{r.title}</h1>
        <div className="actions">
          <button
            type="button"
            className="button ghost"
            aria-pressed={panel}
            onClick={() => setPanel((p) => !p)}
            title="Show or hide ingredients (I)"
          >
            <ListChecks size={18} aria-hidden /> Ingredients
          </button>
          <button type="button" className="button ghost" onClick={exit} title="Exit (Esc)">
            <X size={18} aria-hidden /> Exit
          </button>
        </div>
      </header>

      <div className={panel ? "cook-main with-panel" : "cook-main"}>
        {panel && (
          <aside className="cook-ingredients" aria-label="Ingredients">
            <h2>Ingredients</h2>
            {r.ingredients.length === 0 && <p className="muted">No ingredients.</p>}
            <ul>
              {r.ingredients.map((l, i) =>
                l.is_heading ? (
                  <li key={i} className="heading">
                    {l.item}
                  </li>
                ) : (
                  <Ingredient key={i} line={l} done={checked.has(i)} onToggle={() => toggle(i)} />
                ),
              )}
            </ul>
          </aside>
        )}

        <section className="cook-step" aria-live="polite">
          {count === 0 ? (
            <p className="muted">This recipe has no steps yet.</p>
          ) : (
            <>
              <p className="cook-count">
                Step {step + 1} of {count}
              </p>
              <p className="cook-text">{r.steps[step]}</p>
            </>
          )}
        </section>
      </div>

      <footer className="cook-nav">
        <button type="button" className="button big" disabled={step === 0} onClick={prev}>
          <ChevronLeft size={22} aria-hidden /> Previous
        </button>
        <div className="cook-dots">
          {r.steps.map((_, i) => (
            <button
              key={i}
              type="button"
              className={i === step ? "on" : i < step ? "done" : ""}
              aria-label={`Step ${i + 1}`}
              aria-current={i === step ? "step" : undefined}
              onClick={() => setStep(i)}
            />
          ))}
        </div>
        {step < last ? (
          <button type="button" className="button primary big" onClick={next}>
            Next <ChevronRight size={22} aria-hidden />
          </button>
        ) : (
          <button type="button" className="button primary big" onClick={exit}>
            <Check size={22} aria-hidden /> Done
          </button>
        )}
      </footer>
      <p className="cook-keys">← → move between steps · I shows ingredients · Esc exits</p>
    </div>
  );
}

function Ingredient({
  line,
  done,
  onToggle,
}: {
  line: IngredientLine;
  done: boolean;
  onToggle: () => void;
}) {
  const amount = [line.quantity_text, line.unit].filter(Boolean).join(" ");
  return (
    <li className={done ? "done" : undefined}>
      <label>
        <input type="checkbox" checked={done} onChange={onToggle} />
        <span>
          {amount && <span className="qty">{amount} </span>}
          {line.item}
          {line.prep_note && <span className="muted">, {line.prep_note}</span>}
        </span>
      </label>
    </li>
  );
}
