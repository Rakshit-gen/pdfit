import { useState } from "react";
import { formatBytes, parseLimit } from "./format";

const PRESETS = [1_000_000, 2_000_000, 5_000_000, 10_000_000];

interface Props {
  limit: number | null;
  onChange: (limit: number | null) => void;
  disabled?: boolean;
}

/** The four limits upload forms ask for most, or any other size. */
export default function LimitPicker({ limit, onChange, disabled }: Props) {
  const [custom, setCustom] = useState("");
  const [usingCustom, setUsingCustom] = useState(false);
  const bad = usingCustom && custom.trim() !== "" && parseLimit(custom) === null;

  return (
    <fieldset className="limits" disabled={disabled}>
      <legend>Upload limit</legend>
      <div className="options">
        {PRESETS.map((p) => (
          <label key={p} className="option">
            <input
              type="radio"
              name="limit"
              checked={!usingCustom && limit === p}
              onChange={() => {
                setUsingCustom(false);
                onChange(p);
              }}
            />
            <span>{formatBytes(p).replace(".00", "")}</span>
          </label>
        ))}
        <label className="option">
          <input
            type="radio"
            name="limit"
            checked={usingCustom}
            onChange={() => {
              setUsingCustom(true);
              onChange(parseLimit(custom));
            }}
          />
          <span>Other</span>
        </label>
        {usingCustom && (
          <input
            className="custom"
            type="text"
            inputMode="decimal"
            placeholder="e.g. 3.5 MB or 800 KB"
            aria-label="Custom limit"
            aria-invalid={bad}
            aria-describedby="limit-help"
            autoFocus
            value={custom}
            onChange={(e) => {
              setCustom(e.target.value);
              onChange(parseLimit(e.target.value));
            }}
          />
        )}
      </div>
      <p id="limit-help" className={bad ? "hint error" : "hint"}>
        {bad
          ? "Type a size like 3.5 MB or 800 KB. Anything under 10 KB is too small for a PDF."
          : "1 MB here is 1,000,000 bytes, the stricter way to count, so the file passes either way."}
      </p>
    </fieldset>
  );
}
