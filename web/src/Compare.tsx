import { useEffect, useRef, useState } from "react";
import { GlobalWorkerOptions, getDocument, type PDFDocumentProxy } from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";

GlobalWorkerOptions.workerSrc = workerUrl;

/** Pages are rendered this wide, so the loupe has real pixels to show at
 * 4x on a typical half-screen panel. */
const RENDER_WIDTH = 1800;
const LOUPE = 200;
const ZOOMS = [2, 3, 4];

interface Props {
  before: ArrayBuffer;
  after: ArrayBuffer;
  pages: number;
  startPage: number;
}

interface Spot {
  x: number;
  y: number;
}

/** The same page from both files side by side. Pointing at either one
 * magnifies that spot in both, so a change in a scan or photo shows up. */
export default function Compare({ before, after, pages, startPage }: Props) {
  const [page, setPage] = useState(startPage);
  const [spot, setSpot] = useState<Spot | null>(null);
  const [zoom, setZoom] = useState(3);
  const [docs, setDocs] = useState<[PDFDocumentProxy, PDFDocumentProxy] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    // pdf.js moves the buffer to its worker, so give it copies.
    const tasks = [getDocument({ data: before.slice(0) }), getDocument({ data: after.slice(0) })];
    Promise.all(tasks.map((t) => t.promise))
      .then(([a, b]) => !cancelled && setDocs([a!, b!]))
      .catch((e) => !cancelled && setError(`The preview couldn't open the file (${e.message}). The download is unaffected.`));
    return () => {
      cancelled = true;
      tasks.forEach((t) => void t.destroy());
    };
  }, [before, after]);

  const onKey = (e: React.KeyboardEvent) => {
    const step = e.shiftKey ? 0.1 : 0.02;
    const move = (dx: number, dy: number) => {
      e.preventDefault();
      setSpot((s) => {
        const c = s ?? { x: 0.5, y: 0.5 };
        return { x: clamp(c.x + dx), y: clamp(c.y + dy) };
      });
    };
    if (e.key === "ArrowLeft") move(-step, 0);
    else if (e.key === "ArrowRight") move(step, 0);
    else if (e.key === "ArrowUp") move(0, -step);
    else if (e.key === "ArrowDown") move(0, step);
    else if (e.key === "+" || e.key === "=") setZoom((z) => Math.min(4, z + 1));
    else if (e.key === "-") setZoom((z) => Math.max(2, z - 1));
    else if (e.key === "Escape") setSpot(null);
    else if (e.key === "PageDown" || e.key === "n") turn(1);
    else if (e.key === "PageUp" || e.key === "p") turn(-1);
  };
  const turn = (d: number) => setPage((p) => Math.min(pages, Math.max(1, p + d)));

  return (
    <section className="compare" aria-labelledby="compare-title">
      <div className="compare-head">
        <h2 id="compare-title">Before and after</h2>
        <div className="pager">
          <button type="button" className="secondary" onClick={() => turn(-1)} disabled={page <= 1}>
            Previous page
          </button>
          <span aria-live="polite">
            Page {page} of {pages}
          </span>
          <button type="button" className="secondary" onClick={() => turn(1)} disabled={page >= pages}>
            Next page
          </button>
        </div>
        <label className="zoom">
          Magnify
          <select value={zoom} onChange={(e) => setZoom(Number(e.target.value))}>
            {ZOOMS.map((z) => (
              <option key={z} value={z}>
                {z}x
              </option>
            ))}
          </select>
        </label>
      </div>
      <p className="hint">
        Point at a page to magnify the same spot in both. Or focus the pages and use the arrow keys; + and - change the
        magnification, Escape hides it.
      </p>
      {error && <p className="note error">{error}</p>}
      <div
        className="panes"
        tabIndex={0}
        aria-label={`Page ${page}, original and smaller version. Arrow keys move the magnifier.`}
        onKeyDown={onKey}
        onPointerLeave={(e) => e.pointerType === "mouse" && setSpot(null)}
      >
        <Pane label="Original" doc={docs?.[0]} page={page} spot={spot} zoom={zoom} onSpot={setSpot} />
        <Pane label="Smaller" doc={docs?.[1]} page={page} spot={spot} zoom={zoom} onSpot={setSpot} />
      </div>
    </section>
  );
}

function clamp(v: number) {
  return Math.min(1, Math.max(0, v));
}

interface PaneProps {
  label: string;
  doc: PDFDocumentProxy | undefined;
  page: number;
  spot: Spot | null;
  zoom: number;
  onSpot: (s: Spot) => void;
}

function Pane({ label, doc, page, spot, zoom, onSpot }: PaneProps) {
  const pageCanvas = useRef<HTMLCanvasElement>(null);
  const loupe = useRef<HTMLCanvasElement>(null);
  const [rendering, setRendering] = useState(true);
  const [rendered, setRendered] = useState(0);

  useEffect(() => {
    if (!doc || !pageCanvas.current) return;
    let task: ReturnType<Awaited<ReturnType<PDFDocumentProxy["getPage"]>>["render"]> | null = null;
    let cancelled = false;
    setRendering(true);
    doc.getPage(page).then((p) => {
      if (cancelled || !pageCanvas.current) return;
      const base = p.getViewport({ scale: 1 });
      const viewport = p.getViewport({ scale: RENDER_WIDTH / base.width });
      const canvas = pageCanvas.current;
      canvas.width = Math.round(viewport.width);
      canvas.height = Math.round(viewport.height);
      task = p.render({ canvas, viewport });
      task.promise
        .then(() => {
          if (!cancelled) {
            setRendering(false);
            setRendered((n) => n + 1);
          }
        })
        .catch(() => {
          // Cancelled by a page turn; the next render takes over.
        });
    });
    return () => {
      cancelled = true;
      task?.cancel();
    };
  }, [doc, page]);

  useEffect(() => {
    const src = pageCanvas.current;
    const dst = loupe.current;
    if (!src || !dst || !spot) return;
    const ratio = window.devicePixelRatio || 1;
    dst.width = LOUPE * ratio;
    dst.height = LOUPE * ratio;
    const shown = src.getBoundingClientRect().width || 1;
    // Source pixels covered by the loupe at this zoom.
    const size = (LOUPE / zoom) * (src.width / shown);
    const ctx = dst.getContext("2d");
    if (!ctx) return;
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, dst.width, dst.height);
    ctx.imageSmoothingQuality = "high";
    ctx.drawImage(src, spot.x * src.width - size / 2, spot.y * src.height - size / 2, size, size, 0, 0, dst.width, dst.height);
  }, [spot, zoom, rendered]);

  const point = (e: React.PointerEvent) => {
    const r = e.currentTarget.getBoundingClientRect();
    onSpot({ x: clamp((e.clientX - r.left) / r.width), y: clamp((e.clientY - r.top) / r.height) });
  };

  return (
    <figure className="pane">
      <figcaption>{label}</figcaption>
      <div className="sheet" onPointerMove={point} onPointerDown={point}>
        <canvas ref={pageCanvas} aria-hidden="true" />
        {rendering && <span className="rendering">Rendering page…</span>}
        {spot && (
          <canvas
            ref={loupe}
            className="loupe"
            aria-hidden="true"
            style={{
              width: LOUPE,
              height: LOUPE,
              left: `calc(${spot.x * 100}% - ${LOUPE / 2}px)`,
              top: `calc(${spot.y * 100}% - ${LOUPE / 2}px)`,
            }}
          />
        )}
      </div>
    </figure>
  );
}
