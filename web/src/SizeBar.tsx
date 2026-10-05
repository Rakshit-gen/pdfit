import type { Breakdown } from "./api";
import { formatBytes } from "./format";

interface Props {
  label: string;
  total: number;
  breakdown: Breakdown;
  limit: number | null;
  /** Bytes the bar's full width stands for, shared by the before and after
   * bars so they compare at a glance. */
  scale: number;
}

const PARTS = [
  ["images", "Photos and scans"],
  ["images_kept", "Images left as they are"],
  ["fonts", "Fonts"],
  ["content", "Text and drawings"],
  ["other", "Structure and metadata"],
] as const;

/** Where the bytes go, drawn to scale, with the limit as a line. */
export default function SizeBar({ label, total, breakdown, limit, scale }: Props) {
  const known = breakdown.images + breakdown.images_kept + breakdown.fonts + breakdown.content;
  const parts = { ...breakdown, other: Math.max(0, total - known) };
  const pct = (n: number) => `${(100 * n) / scale}%`;
  const over = limit !== null && total > limit;

  return (
    <figure className="sizebar">
      <figcaption>
        <span>{label}</span>
        <strong className={over ? "over" : limit !== null ? "under" : undefined}>{formatBytes(total)}</strong>
      </figcaption>
      <div className="bar" role="img" aria-label={describe(label, total, parts, limit)}>
        {PARTS.map(([key]) =>
          parts[key] > 0 ? <span key={key} className={`seg ${key}`} style={{ width: pct(parts[key]) }} /> : null,
        )}
        {limit !== null && <span className="limit" style={{ left: pct(limit) }} />}
      </div>
    </figure>
  );
}

export function SizeLegend({ breakdown }: { breakdown: Breakdown }) {
  return (
    <ul className="legend">
      {PARTS.filter(([key]) => key === "other" || breakdown[key] > 0).map(([key, name]) => (
        <li key={key}>
          <span className={`swatch ${key}`} aria-hidden="true" />
          {name}
        </li>
      ))}
      <li>
        <span className="swatch limit" aria-hidden="true" />
        Limit
      </li>
    </ul>
  );
}

function describe(label: string, total: number, parts: Breakdown, limit: number | null): string {
  const bits = PARTS.filter(([k]) => parts[k] > 0).map(([k, name]) => `${name} ${formatBytes(parts[k])}`);
  const lim = limit === null ? "" : `, limit ${formatBytes(limit)}`;
  return `${label}: ${formatBytes(total)}${lim}. ${bits.join(", ")}.`;
}
