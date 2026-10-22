import { useId, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { X } from "lucide-react";
import { api, queryKeys } from "../api";
import type { LabelKind } from "../bindings/LabelKind";

const MAX_SUGGESTIONS = 50;

type Option = { name: string; isNew: boolean };

const lower = (s: string) => s.toLocaleLowerCase();
const tidy = (s: string) => s.trim().replace(/\s+/g, " ");

/**
 * Chips plus a type-ahead combobox. Enter adds the highlighted suggestion (or creates a
 * new name), comma adds exactly what was typed, Backspace on an empty input removes the last chip. Unknown names
 * are created by Rust when the recipe is saved.
 */
export default function LabelPicker({
  kind,
  label,
  value,
  onChange,
}: {
  kind: LabelKind;
  label: string;
  value: string[];
  onChange: (names: string[]) => void;
}) {
  const known = useQuery({ queryKey: queryKeys.labels(kind), queryFn: () => api.listLabels(kind) });
  const [text, setText] = useState("");
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const id = useId();

  const typed = tidy(text);
  const chosen = new Set(value.map(lower));
  const names = (known.data ?? []).map((l) => l.name);
  const matches = names
    .filter((n) => !chosen.has(lower(n)) && lower(n).includes(lower(typed)))
    .sort(
      (a, b) =>
        Number(!lower(b).startsWith(lower(typed))) - Number(!lower(a).startsWith(lower(typed))),
    )
    .slice(0, MAX_SUGGESTIONS);
  const exists = names.some((n) => lower(n) === lower(typed)) || chosen.has(lower(typed));
  const options: Option[] = [
    ...matches.map((name) => ({ name, isNew: false })),
    ...(typed && !exists ? [{ name: typed, isNew: true }] : []),
  ];
  const showList = open && options.length > 0;
  const activeIndex = Math.min(active, options.length - 1);

  const add = (raw: string) => {
    const name = tidy(raw);
    if (name && !chosen.has(lower(name))) {
      // Reuse the existing spelling when the name is already known.
      onChange([...value, names.find((n) => lower(n) === lower(name)) ?? name]);
    }
    setText("");
    setActive(0);
  };

  const remove = (name: string) => onChange(value.filter((n) => n !== name));

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        setOpen(true);
        setActive(Math.min(activeIndex + 1, options.length - 1));
        break;
      case "ArrowUp":
        e.preventDefault();
        setActive(Math.max(activeIndex - 1, 0));
        break;
      case "Enter":
        // Never submit the form from here.
        e.preventDefault();
        if (showList) add(options[activeIndex].name);
        else if (typed) add(typed);
        break;
      case ",":
        e.preventDefault();
        if (typed) add(typed);
        break;
      case "Backspace":
        if (text === "" && value.length > 0) remove(value[value.length - 1]);
        break;
      case "Escape":
        if (showList) {
          e.preventDefault();
          setOpen(false);
        }
        break;
    }
  };

  return (
    <div className="label">
      <span id={`${id}-label`}>{label}</span>
      <div className="label-picker" onClick={() => inputRef.current?.focus()}>
        {value.map((name) => (
          <span key={name} className={`chip ${kind === "tag" ? "tag" : ""}`}>
            {name}
            <button
              type="button"
              className="chip-remove"
              aria-label={`Remove ${name}`}
              onClick={(e) => {
                e.stopPropagation();
                remove(name);
              }}
            >
              <X size={12} aria-hidden />
            </button>
          </span>
        ))}
        <input
          ref={inputRef}
          value={text}
          role="combobox"
          aria-labelledby={`${id}-label`}
          aria-expanded={showList}
          aria-controls={`${id}-list`}
          aria-autocomplete="list"
          aria-activedescendant={showList ? `${id}-opt-${activeIndex}` : undefined}
          placeholder={value.length === 0 ? `Add ${kind === "tag" ? "tags" : "categories"}…` : ""}
          onChange={(e) => {
            setText(e.target.value);
            setActive(0);
            setOpen(true);
          }}
          onFocus={() => setOpen(true)}
          onBlur={() => {
            // Keep what was typed rather than silently dropping it.
            if (typed) add(typed);
            setOpen(false);
          }}
          onKeyDown={onKeyDown}
        />
        {showList && (
          <ul className="label-options" role="listbox" id={`${id}-list`}>
            {options.map((o, i) => (
              <li
                key={`${o.isNew}-${o.name}`}
                id={`${id}-opt-${i}`}
                role="option"
                aria-selected={i === activeIndex}
                className={i === activeIndex ? "active" : ""}
                // mousedown, not click: keep focus in the input so blur doesn't fire first.
                onMouseDown={(e) => {
                  e.preventDefault();
                  add(o.name);
                }}
                onMouseEnter={() => setActive(i)}
              >
                {o.isNew ? (
                  <>
                    Create <strong>“{o.name}”</strong>
                  </>
                ) : (
                  o.name
                )}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
