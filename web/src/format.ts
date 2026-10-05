// Sizes use the same units as the command line: 1 MB is 1,000,000 bytes,
// the stricter reading, so a result under "2 MB" passes either way a site
// counts.

export function formatBytes(n: number): string {
  if (n >= 1e6) return `${(n / 1e6).toFixed(2).replace(/\.00$/, "")} MB`;
  if (n >= 1e3) return `${Math.round(n / 1e3)} KB`;
  return `${n} bytes`;
}

/** Reads "2", "2 MB", "1.5mb", "500 KB" or "500k". Bare numbers are MB,
 * because that's what people type into the custom box. */
export function parseLimit(text: string): number | null {
  const m = text.trim().toLowerCase().match(/^(\d+(?:[.,]\d+)?)\s*(mb|m|kb|k)?$/);
  if (!m) return null;
  const n = parseFloat(m[1]!.replace(",", "."));
  const unit = m[2] ?? "mb";
  const bytes = Math.floor(n * (unit.startsWith("k") ? 1e3 : 1e6));
  return bytes >= 10_000 ? bytes : null;
}

export function percent(part: number, whole: number): string {
  if (whole <= 0) return "0%";
  const p = (100 * part) / whole;
  if (p > 0 && p < 1) return "<1%";
  return `${Math.round(p)}%`;
}

export function pageList(pages: number[]): string {
  if (pages.length === 0) return "not drawn";
  if (pages.length === 1) return `${pages[0]}`;
  return `${pages[0]}-${pages[pages.length - 1]}`;
}

/** The page to compare first: the one holding the most image bytes, since
 * that's where any change will show. */
export function heaviestPage(images: { pages: number[]; bytes: number }[]): number {
  const weight = new Map<number, number>();
  for (const img of images) {
    for (const p of img.pages) weight.set(p, (weight.get(p) ?? 0) + img.bytes);
  }
  let best = 1;
  let most = -1;
  for (const [p, w] of weight) {
    if (w > most || (w === most && p < best)) {
      best = p;
      most = w;
    }
  }
  return best;
}

/** report.pdf becomes report.small.pdf, like the command line. */
export function smallName(name: string): string {
  const stem = name.replace(/\.pdf$/i, "");
  return `${stem || "document"}.small.pdf`;
}
