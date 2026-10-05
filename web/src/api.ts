// Shapes of what pdfit-web sends back. They mirror the Rust structs in
// src/doc.rs and src/fit.rs.

export interface ImageInfo {
  obj: number;
  pages: number[];
  width: number;
  height: number;
  format: string;
  color: string;
  bytes: number;
  /** Largest drawn size in inches, width by height. */
  drawn: [number, number] | null;
  /** Effective dpi where it's drawn largest. */
  dpi: number | null;
  has_mask: boolean;
  /** Why pdfit leaves it alone, or null if it can be recompressed. */
  skip: string | null;
}

export interface Breakdown {
  images: number;
  images_kept: number;
  fonts: number;
  content: number;
  other: number;
}

export interface Inspection {
  name: string;
  bytes: number;
  pages: number;
  inline_images: number;
  images: ImageInfo[];
  breakdown: Breakdown;
}

export interface Setting {
  dpi: number;
  quality: number;
}

export interface Change {
  before: number;
  after: number;
  width: number;
  height: number;
}

export interface Outcome {
  input_bytes: number;
  output_bytes: number;
  limit: number;
  fits: boolean;
  already_fits: boolean;
  setting: Setting | null;
  attempts: { setting: Setting | null; bytes: number; fits: boolean }[];
  images: (ImageInfo & { after: Change | null })[];
  breakdown: Breakdown;
  reason: string | null;
  millis: number;
}

/** An error the server explained. `kind` is "encrypted", "not_pdf",
 * "broken" and so on; the message is written for people. */
export class ApiError extends Error {
  constructor(
    message: string,
    readonly kind: string,
  ) {
    super(message);
  }
}

async function check(res: Response): Promise<Response> {
  if (res.ok) return res;
  let body: { error?: string; kind?: string } = {};
  try {
    body = await res.json();
  } catch {
    // Not one of ours: a proxy error page or similar.
  }
  throw new ApiError(body.error ?? `The server answered ${res.status} ${res.statusText}.`, body.kind ?? "server");
}

async function send(url: string, init: RequestInit): Promise<Response> {
  try {
    return await check(await fetch(url, init));
  } catch (e) {
    if (e instanceof ApiError) throw e;
    throw new ApiError("Can't reach pdfit. Check that pdfit-web is still running, then try again.", "offline");
  }
}

export async function inspect(file: File): Promise<Inspection> {
  const res = await send("/api/inspect", {
    method: "POST",
    headers: { "X-Filename": encodeURIComponent(file.name) },
    body: file,
  });
  return res.json();
}

export async function fit(limit: number): Promise<Outcome> {
  const res = await send("/api/fit", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ limit }),
  });
  return res.json();
}

export async function result(): Promise<ArrayBuffer> {
  return (await send("/api/result", {})).arrayBuffer();
}
