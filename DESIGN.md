# Design notes

pdfit is used at an awkward moment: a form somewhere has said "max 2 MB" and the scan of a passport, a payslip or a
signed contract is 6 MB. The page should feel like the paperwork it handles, calm and plain, and show the one thing
that matters right away: how big the file is next to the limit.

The look is paper and ink. A warm off-white page, sheets with thin ruled borders, black type, a blue pen for actions
and a red pen for the limit. No dark mode for now; documents are read on white, and the before and after renders
should sit on the same ground as the paper they came from.

## Colour

| Token | Value | Job |
| --- | --- | --- |
| `--paper` | `#f3f0e8` | Page ground, like a desk blotter. |
| `--sheet` | `#fcfbf7` | Panels, the drop zone, the table. |
| `--rule` / `--rule-strong` | `#d8d1c2` / `#b9b09d` | Borders, table rules, the ticks on the size bar. |
| `--ink` | `#1f2328` | Text, table heads, the selected limit. 13.9:1 on paper. |
| `--muted` | `#5b5850` | Secondary text. 6.2:1 on paper, 6.9:1 on a sheet. |
| `--pen` | `#2350a8` | Photos and scans in the size bar, and the buttons that act. White on it is 7.5:1. |
| `--red` | `#b3261e` | The limit line, a size over the limit, and a result that didn't fit. 6.3:1 on a sheet. |
| `--green` | `#2b6a3c` | A size under the limit and a result that fits. 6.3:1 on a sheet. |
| `--ochre` | `#a8741a` | Fonts in the size bar, the main reason a file can't shrink. |

Blue always means "the part pdfit can change": the photo segment of the bar, the share bars in the image table and
the primary button. Red always means the limit. A file's size turns red or green depending on which side of the limit
it lands, so the verdict is readable before the sentence under it.

## Type

Public Sans at 400 and 600, bundled with @fontsource. It was drawn for government forms and sites, which is the world
these files are going to, and its figures are clear at small sizes. Tabular figures everywhere so the image table's
sizes and dpi line up.

## Layout

One column of sheets, top to bottom in the order of the job: the file and where its bytes go, the limit and the
button, the images, then the pages before and after.

The signature piece is the size bar. It draws the file to scale, split into photos, images pdfit leaves alone, fonts,
text and drawings, and structure, with the limit as a red line through it. Before and after share one scale, so the
result is visibly shorter, and when fonts alone cross the line the reason it can't fit is visible before it is read.

The comparison puts the original and the smaller file side by side, rendered with pdf.js, and a round magnifier shows
the same spot in both at 2x to 4x. Below 720 px the pages stack and the share column of the table is dropped.

## Motion

Only state changes move: the drop zone border when a file is over it, and the size bar segments when a new result
arrives. Both stop under `prefers-reduced-motion`.

## Voice

The page talks to someone who needs to upload a document and may not know what dpi or JPEG quality mean, but deserves
to see them. It leads with sizes and the limit, says what changed in a sentence, and says plainly that text and fonts
were not touched, since that is what people worry about with documents. When a file can't fit it says what is taking
the space and that pdfit won't touch it, instead of offering a broken result as a success. Errors name the problem and
the next step. Sentence case throughout.
