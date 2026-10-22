import { type KeyboardEvent, useEffect, useRef } from "react";
import { Plus } from "lucide-react";
import SortableList from "./SortableList";
import { RemoveButton } from "./IngredientsEditor";

export type StepRow = { key: string; text: string };

export function blankStep(text = ""): StepRow {
  return { key: crypto.randomUUID(), text };
}

type Props = { steps: StepRow[]; onChange: (steps: StepRow[]) => void };

/** Enter adds a step below, Shift+Enter inserts a line break, Backspace removes an empty step. */
export default function StepsEditor({ steps, onChange }: Props) {
  const inputs = useRef(new Map<string, HTMLTextAreaElement>());
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

  const insertAfter = (index: number) => {
    const step = blankStep();
    onChange([...steps.slice(0, index + 1), step, ...steps.slice(index + 1)]);
    focusAfterRender(step.key);
  };

  const remove = (index: number) => {
    onChange(steps.filter((_, i) => i !== index));
    const prev = steps[index - 1];
    if (prev) focusAfterRender(prev.key);
  };

  const onKeyDown = (e: KeyboardEvent, index: number) => {
    if (e.nativeEvent.isComposing) return;
    if (e.key === "Enter" && !e.shiftKey && !e.metaKey && !e.ctrlKey) {
      e.preventDefault();
      insertAfter(index);
    } else if (e.key === "Backspace" && steps[index].text === "" && steps.length > 1) {
      e.preventDefault();
      remove(index);
    }
  };

  return (
    <div className="steps-editor">
      <SortableList
        items={steps}
        onReorder={onChange}
        className="step-rows"
        renderItem={(step, index, handle) => (
          <div className="step-row">
            {handle}
            <span className="step-number">{index + 1}</span>
            <textarea
              ref={(el) => {
                if (el) inputs.current.set(step.key, el);
                else inputs.current.delete(step.key);
              }}
              value={step.text}
              rows={2}
              onChange={(e) =>
                onChange(
                  steps.map((s) => (s.key === step.key ? { ...s, text: e.target.value } : s)),
                )
              }
              onKeyDown={(e) => onKeyDown(e, index)}
              placeholder="Describe this step"
              aria-label={`Step ${index + 1}`}
            />
            <RemoveButton onClick={() => remove(index)} label={`Remove step ${index + 1}`} />
          </div>
        )}
      />
      <div className="row-actions">
        <button type="button" className="button" onClick={() => insertAfter(steps.length - 1)}>
          <Plus size={16} aria-hidden /> Step
        </button>
      </div>
      <p className="hint">Enter adds a step · Shift+Enter for a new line</p>
    </div>
  );
}
