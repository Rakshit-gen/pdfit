import { useRef, useState } from "react";

interface Props {
  onFile: (file: File) => void;
  compact?: boolean;
}

/** A file picker that also takes drops. It's a real button, so Tab and
 * Enter work; dropping anywhere on the page is handled by App. */
export default function DropZone({ onFile, compact }: Props) {
  const input = useRef<HTMLInputElement>(null);
  const [over, setOver] = useState(false);

  return (
    <div
      className={`drop${over ? " over" : ""}${compact ? " compact" : ""}`}
      onDragOver={(e) => {
        e.preventDefault();
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        e.preventDefault();
        e.stopPropagation();
        setOver(false);
        const f = e.dataTransfer.files[0];
        if (f) onFile(f);
      }}
    >
      {!compact && <p className="drop-title">Drop a PDF here</p>}
      <button type="button" className={compact ? "secondary" : "primary"} onClick={() => input.current?.click()}>
        {compact ? "Choose another PDF" : "Choose a PDF"}
      </button>
      {!compact && <p className="hint">It stays on this computer. pdfit-web only listens on 127.0.0.1.</p>}
      <input
        ref={input}
        type="file"
        accept="application/pdf,.pdf"
        hidden
        onChange={(e) => {
          const f = e.target.files?.[0];
          if (f) onFile(f);
          e.target.value = "";
        }}
      />
    </div>
  );
}
