import { lazy, Suspense, useEffect, useState } from "react";
import { ApiError, fit, inspect, result, type Inspection, type Outcome } from "./api";
import DropZone from "./DropZone";
import ImageTable from "./ImageTable";
import LimitPicker from "./LimitPicker";
import SizeBar, { SizeLegend } from "./SizeBar";
import { formatBytes, heaviestPage, smallName } from "./format";

// pdf.js is most of the page's weight; load it once there's something to show.
const Compare = lazy(() => import("./Compare"));

type Phase =
  | { k: "empty" }
  | { k: "reading"; name: string }
  | { k: "failed"; name: string; kind: string; message: string }
  | { k: "ready"; file: File; info: Inspection; error?: string }
  | { k: "working"; file: File; info: Inspection }
  | { k: "done"; file: File; info: Inspection; outcome: Outcome; before: ArrayBuffer; after: ArrayBuffer };

const FAILURE_TITLES: Record<string, string> = {
  encrypted: "This PDF is password-protected",
  not_pdf: "That file isn't a PDF",
  broken: "This PDF can't be read",
  empty: "That file is empty",
  offline: "pdfit isn't answering",
};

export default function App() {
  const [phase, setPhase] = useState<Phase>({ k: "empty" });
  const [limit, setLimit] = useState<number | null>(2_000_000);

  const open = async (file: File) => {
    setPhase({ k: "reading", name: file.name });
    try {
      const info = await inspect(file);
      setPhase({ k: "ready", file, info });
    } catch (e) {
      const err = e instanceof ApiError ? e : new ApiError(String(e), "broken");
      setPhase({ k: "failed", name: file.name, kind: err.kind, message: err.message });
    }
  };

  // Dropping a file anywhere on the page works, not only on the drop box.
  useEffect(() => {
    const over = (e: DragEvent) => e.preventDefault();
    const drop = (e: DragEvent) => {
      e.preventDefault();
      const f = e.dataTransfer?.files[0];
      if (f) void open(f);
    };
    window.addEventListener("dragover", over);
    window.addEventListener("drop", drop);
    return () => {
      window.removeEventListener("dragover", over);
      window.removeEventListener("drop", drop);
    };
  }, []);

  const run = async () => {
    if ((phase.k !== "ready" && phase.k !== "done") || limit === null) return;
    const { file, info } = phase;
    setPhase({ k: "working", file, info });
    try {
      const outcome = await fit(limit);
      const [before, after] = await Promise.all([file.arrayBuffer(), result()]);
      setPhase({ k: "done", file, info, outcome, before, after });
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      setPhase({ k: "ready", file, info, error: message });
    }
  };

  const changeLimit = (l: number | null) => {
    setLimit(l);
    // A result for another limit would be misleading next to the new one.
    if (phase.k === "done") setPhase({ k: "ready", file: phase.file, info: phase.info });
  };

  return (
    <div className="page">
      <header className="masthead">
        <img src="/logo.svg" alt="" width="36" height="36" />
        <div>
          <h1>pdfit</h1>
          <p>
            Shrink a PDF to fit an upload limit. Only photos and scans are recompressed; text, fonts and drawings stay
            exactly as they were.
          </p>
        </div>
      </header>

      <main>
        {phase.k === "empty" && <DropZone onFile={open} />}

        {phase.k === "reading" && (
          <section className="status" aria-live="polite">
            <p>
              Reading <strong>{phase.name}</strong>…
            </p>
          </section>
        )}

        {phase.k === "failed" && (
          <section className="status failed" role="alert">
            <h2>{FAILURE_TITLES[phase.kind] ?? "Something went wrong"}</h2>
            <p className="file-name">{phase.name}</p>
            <p>{phase.message}</p>
            <DropZone onFile={open} compact />
          </section>
        )}

        {(phase.k === "ready" || phase.k === "working" || phase.k === "done") && (
          <Workspace
            phase={phase}
            limit={limit}
            onLimit={changeLimit}
            onRun={run}
            onFile={open}
          />
        )}
      </main>
    </div>
  );
}

interface WorkspaceProps {
  phase: Extract<Phase, { k: "ready" | "working" | "done" }>;
  limit: number | null;
  onLimit: (l: number | null) => void;
  onRun: () => void;
  onFile: (f: File) => void;
}

function Workspace({ phase, limit, onLimit, onRun, onFile }: WorkspaceProps) {
  const { info } = phase;
  const outcome = phase.k === "done" ? phase.outcome : null;
  const scale = Math.max(info.bytes, limit ?? 0) * 1.04;
  const working = phase.k === "working";

  return (
    <>
      <section className="file" aria-labelledby="file-title">
        <div className="file-head">
          <div>
            <h2 id="file-title" className="file-name">
              {info.name}
            </h2>
            <p className="muted">
              {formatBytes(info.bytes)}, {info.pages} {info.pages === 1 ? "page" : "pages"}, {info.images.length}{" "}
              {info.images.length === 1 ? "image" : "images"}
            </p>
          </div>
          <DropZone onFile={onFile} compact />
        </div>
        <SizeBar label="Original" total={info.bytes} breakdown={info.breakdown} limit={limit} scale={scale} />
        {outcome && (
          <SizeBar label="Smaller" total={outcome.output_bytes} breakdown={outcome.breakdown} limit={limit} scale={scale} />
        )}
        <SizeLegend breakdown={info.breakdown} />
      </section>

      <section className="controls" aria-label="Shrink">
        <LimitPicker limit={limit} onChange={onLimit} disabled={working} />
        <button type="button" className="primary" onClick={onRun} disabled={working || limit === null}>
          {working ? "Shrinking…" : limit === null ? "Pick a limit" : `Shrink to under ${formatBytes(limit)}`}
        </button>
        <div aria-live="polite" className="live">
          {working && (
            <p className="muted">
              Trying settings from 300 dpi down until the file fits. Large scans can take a few seconds.
            </p>
          )}
          {phase.k === "ready" && phase.error && <p className="note error">{phase.error}</p>}
          {phase.k === "done" && <Verdict outcome={phase.outcome} name={info.name} after={phase.after} />}
        </div>
      </section>

      <section aria-labelledby="images-title">
        <h2 id="images-title">Images</h2>
        <ImageTable
          images={outcome ? outcome.images : info.images}
          fileBytes={info.bytes}
          done={outcome !== null}
          inlineImages={info.inline_images}
        />
      </section>

      {phase.k === "done" && !phase.outcome.already_fits && (
        <Suspense fallback={<p className="muted">Loading the preview…</p>}>
          <Compare
            before={phase.before}
            after={phase.after}
            pages={info.pages}
            startPage={heaviestPage(info.images.filter((i) => !i.skip))}
          />
        </Suspense>
      )}
    </>
  );
}

function Verdict({ outcome, name, after }: { outcome: Outcome; name: string; after: ArrayBuffer }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    const u = URL.createObjectURL(new Blob([after], { type: "application/pdf" }));
    setUrl(u);
    return () => URL.revokeObjectURL(u);
  }, [after]);

  const o = outcome;
  const seconds = (o.millis / 1000).toFixed(1);
  const download = url && (
    <a className={o.fits ? "button primary" : "button secondary"} href={url} download={smallName(name)}>
      {o.fits ? `Download ${smallName(name)}` : `Download the smallest version anyway (${formatBytes(o.output_bytes)})`}
    </a>
  );

  if (o.already_fits) {
    return (
      <div className="verdict fits">
        <p>
          It's already under {formatBytes(o.limit)}, so there's nothing to do. You can upload the original as it is.
        </p>
      </div>
    );
  }
  if (!o.fits) {
    return (
      <div className="verdict short" role="alert">
        <h3>Couldn't get it under {formatBytes(o.limit)}</h3>
        <p>{o.reason}</p>
        <p className="muted">Text, fonts and drawings are unchanged in every version pdfit made.</p>
        {download}
      </div>
    );
  }
  return (
    <div className="verdict fits">
      <h3>
        {formatBytes(o.input_bytes)} to {formatBytes(o.output_bytes)}, under the {formatBytes(o.limit)} limit
      </h3>
      <p>
        {o.setting
          ? `Images at up to ${o.setting.dpi} dpi, JPEG quality ${o.setting.quality}, the best of ${o.attempts.length} versions tried.`
          : "Lossless clean-up was enough; no image was recompressed."}{" "}
        Text, fonts and drawings are byte-for-byte unchanged. Took {seconds} s.
      </p>
      {download}
    </div>
  );
}
