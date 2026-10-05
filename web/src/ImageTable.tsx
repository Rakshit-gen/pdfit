import type { Change, ImageInfo } from "./api";
import { formatBytes, pageList, percent } from "./format";

interface Props {
  images: (ImageInfo & { after?: Change | null })[];
  fileBytes: number;
  done: boolean;
  inlineImages: number;
}

/** Every image, heaviest first, with what it costs and what became of it. */
export default function ImageTable({ images, fileBytes, done, inlineImages }: Props) {
  if (images.length === 0) {
    return (
      <p className="note">
        This PDF has no images pdfit can see{inlineImages > 0 ? ` apart from ${inlineImages} small inline ones` : ""}, so
        there's little to shrink.
      </p>
    );
  }
  return (
    <div className="table-wrap">
      <table className="images">
        <thead>
          <tr>
            <th scope="col">Page</th>
            <th scope="col" className="num">Pixels</th>
            <th scope="col" className="num">Drawn at</th>
            <th scope="col" className="num">Size</th>
            <th scope="col">Share of file</th>
            {done && <th scope="col" className="num">After</th>}
          </tr>
        </thead>
        <tbody>
          {images.map((img) => (
            <tr key={img.obj}>
              <td>{pageList(img.pages)}</td>
              <td className="num">
                {img.width} × {img.height}
                <span className="sub">
                  {img.format} {img.color}
                  {img.has_mask ? ", with mask" : ""}
                </span>
              </td>
              <td className="num">{img.dpi === null ? "" : `${Math.round(img.dpi)} dpi`}</td>
              <td className="num">{formatBytes(img.bytes)}</td>
              <td>
                <span className="share">
                  <span className="share-bar" style={{ width: percent(img.bytes, fileBytes).replace("<", "") }} />
                </span>
                {percent(img.bytes, fileBytes)}
              </td>
              {done && <td className="num">{after(img)}</td>}
            </tr>
          ))}
        </tbody>
      </table>
      {images.some((i) => i.skip) && (
        <ul className="skips">
          {images
            .filter((i) => i.skip)
            .map((i) => (
              <li key={i.obj}>
                Page {pageList(i.pages)}, {i.width} × {i.height}: {i.skip}.
              </li>
            ))}
        </ul>
      )}
      {inlineImages > 0 && <p className="note">Plus {inlineImages} inline images inside page content, which pdfit never changes.</p>}
    </div>
  );
}

function after(img: { skip: string | null; after?: Change | null }) {
  if (img.skip) return <span className="muted">unchanged</span>;
  if (!img.after) return <span className="muted">kept</span>;
  return (
    <>
      {formatBytes(img.after.after)}
      <span className="sub">
        {img.after.width} × {img.after.height}
      </span>
    </>
  );
}
