import { AlertCircle, ImageOff, Star } from "lucide-react";

export function ErrorBanner({ message }: { message: string }) {
  return (
    <div className="error-banner" role="alert">
      <AlertCircle size={18} aria-hidden />
      <span>{message}</span>
    </div>
  );
}

export function PhotoPlaceholder() {
  return (
    <div className="photo-placeholder" aria-hidden>
      <ImageOff size={28} />
    </div>
  );
}

export function Stars({ value }: { value: number }) {
  return (
    <span className="stars" aria-label={`${value} out of 5 stars`}>
      {[1, 2, 3, 4, 5].map((n) => (
        <Star key={n} size={14} className={n <= value ? "on" : "off"} aria-hidden />
      ))}
    </span>
  );
}

export function StarPicker({
  value,
  onChange,
}: {
  value: number | null;
  onChange: (v: number | null) => void;
}) {
  return (
    <div className="star-picker" role="radiogroup" aria-label="Rating">
      {[1, 2, 3, 4, 5].map((n) => (
        <button
          key={n}
          type="button"
          role="radio"
          aria-checked={value === n}
          aria-label={`${n} star${n > 1 ? "s" : ""}`}
          title={value === n ? "Click again to clear" : `${n} star${n > 1 ? "s" : ""}`}
          onClick={() => onChange(value === n ? null : n)}
        >
          <Star size={20} className={value !== null && n <= value ? "on" : "off"} aria-hidden />
        </button>
      ))}
    </div>
  );
}

/** Formats a quantity (or range) for display. Friendly fractions arrive in Phase 3. */
export function formatQuantity(q: number | null, max: number | null): string {
  if (q === null) return "";
  const fmt = (n: number) => String(Math.round(n * 1000) / 1000);
  return max !== null ? `${fmt(q)}–${fmt(max)}` : fmt(q);
}

export function formatMinutes(m: number | null): string | null {
  if (m === null) return null;
  const h = Math.floor(m / 60);
  const min = m % 60;
  if (h === 0) return `${min} min`;
  return min === 0 ? `${h} h` : `${h} h ${min} min`;
}
