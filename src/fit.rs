//! Find the gentlest setting that gets a file under a size limit.

use std::collections::HashSet;
use std::time::Instant;

use lopdf::{Document, Object, ObjectId};
use serde::Serialize;

use crate::doc::{self, Error, ImageInfo};
use crate::shrink::{Change, Prepared, Setting};
use crate::verify;

/// Highest quality first. 300 dpi is print quality, 150 is what a document
/// needs on screen, and below 100 small print in scans starts to break up,
/// so the bottom rungs are a last resort.
pub const LADDER: &[Setting] = &[
    Setting {
        dpi: 300,
        quality: 85,
    },
    Setting {
        dpi: 250,
        quality: 80,
    },
    Setting {
        dpi: 200,
        quality: 80,
    },
    Setting {
        dpi: 200,
        quality: 70,
    },
    Setting {
        dpi: 150,
        quality: 70,
    },
    Setting {
        dpi: 150,
        quality: 60,
    },
    Setting {
        dpi: 120,
        quality: 55,
    },
    Setting {
        dpi: 100,
        quality: 50,
    },
    Setting {
        dpi: 72,
        quality: 40,
    },
];

#[derive(Clone, Debug, Serialize)]
pub struct Attempt {
    /// None means lossless tidying only.
    pub setting: Option<Setting>,
    pub bytes: usize,
    pub fits: bool,
}

/// Where the bytes of the result go, from the stored stream sizes.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Breakdown {
    pub images: usize,
    /// Images pdfit leaves as they are (JBIG2, palette, ...).
    pub images_kept: usize,
    pub fonts: usize,
    /// Page content and form streams: the text, lines and layout.
    pub content: usize,
    /// Everything else: structure, metadata, annotations.
    pub other: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImageReport {
    #[serde(flatten)]
    pub info: ImageInfo,
    pub after: Option<Change>,
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub limit: usize,
    pub fits: bool,
    /// The input already fit; it's returned unchanged.
    pub already_fits: bool,
    pub setting: Option<Setting>,
    pub attempts: Vec<Attempt>,
    pub images: Vec<ImageReport>,
    pub tidied: crate::tidy::Tidied,
    pub inline_images: usize,
    pub breakdown: Breakdown,
    /// When it doesn't fit: why, in a sentence.
    pub reason: Option<String>,
    pub millis: u128,
    #[serde(skip)]
    pub output: Vec<u8>,
}

/// Lower is smaller, assuming file size falls as the ladder goes down.
/// It does in practice: each rung lowers dpi or quality and never raises
/// either, and an image only changes when that makes it smaller.
pub fn fit(input: &[u8], limit: usize) -> Result<Outcome, Error> {
    let started = Instant::now();
    let original = doc::load(input)?;
    let before = doc::inventory(&original);
    if input.len() <= limit {
        let breakdown = breakdown(&original, &HashSet::new());
        return Ok(Outcome {
            input_bytes: input.len(),
            output_bytes: input.len(),
            limit,
            fits: true,
            already_fits: true,
            setting: None,
            attempts: Vec::new(),
            images: report(&before.images, &[]),
            tidied: Default::default(),
            inline_images: before.inline_images,
            breakdown,
            reason: None,
            millis: started.elapsed().as_millis(),
            output: input.to_vec(),
        });
    }

    let text_before = verify::fingerprint(&original);
    let prepared = Prepared::new(original);
    let mut attempts = Vec::new();
    let mut tried = |setting: Option<Setting>| -> Result<_, Error> {
        let r = prepared.render(setting)?;
        attempts.push(Attempt {
            setting,
            bytes: r.bytes.len(),
            fits: r.bytes.len() <= limit,
        });
        Ok(r)
    };

    let lossless = tried(None)?;
    let (best, setting) = if lossless.bytes.len() <= limit {
        (lossless, None)
    } else {
        let last = LADDER.len() - 1;
        let floor = tried(Some(LADDER[last]))?;
        if floor.bytes.len() > limit {
            // Doesn't fit at all: hand back the smallest thing made.
            if floor.bytes.len() < lossless.bytes.len() {
                (floor, Some(LADDER[last]))
            } else {
                (lossless, None)
            }
        } else {
            let (mut lo, mut hi, mut best) = (0, last, floor);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let r = tried(Some(LADDER[mid]))?;
                if r.bytes.len() <= limit {
                    hi = mid;
                    best = r;
                } else {
                    lo = mid + 1;
                }
            }
            (best, Some(LADDER[hi]))
        }
    };

    // Re-read what's about to be handed back: it has to parse, and its
    // text, drawings and fonts have to match the input exactly.
    let reread = doc::load(&best.bytes)
        .map_err(|e| Error::Write(format!("the result didn't read back: {e}")))?;
    if verify::fingerprint(&reread) != text_before {
        return Err(Error::Write(
            "the result's text or fonts differ from the original, so it was thrown away. This is a bug in pdfit."
                .into(),
        ));
    }

    let fits = best.bytes.len() <= limit;
    let changed: HashSet<u32> = best.changes.iter().map(|c| c.obj).collect();
    let kept: HashSet<ObjectId> = prepared
        .inventory
        .images
        .iter()
        .filter(|i| i.info.skip.is_some() && !changed.contains(&i.info.obj))
        .map(|i| i.id)
        .collect();
    let breakdown = breakdown(&best.doc, &kept);
    let reason = (!fits).then(|| explain(&breakdown, best.bytes.len(), limit, setting));
    Ok(Outcome {
        input_bytes: input.len(),
        output_bytes: best.bytes.len(),
        limit,
        fits,
        already_fits: false,
        setting,
        attempts,
        images: report(&before.images, &best.changes),
        tidied: prepared.tidied.clone(),
        inline_images: before.inline_images,
        breakdown,
        reason,
        millis: started.elapsed().as_millis(),
        output: best.bytes,
    })
}

fn report(images: &[doc::Image], changes: &[Change]) -> Vec<ImageReport> {
    images
        .iter()
        .map(|i| ImageReport {
            info: i.info.clone(),
            after: changes.iter().find(|c| c.obj == i.info.obj).cloned(),
        })
        .collect()
}

pub fn mb(bytes: usize) -> String {
    let m = bytes as f64 / 1_000_000.0;
    if m >= 10.0 {
        format!("{m:.0} MB")
    } else if m >= 0.1 {
        format!("{m:.1} MB")
    } else {
        format!("{:.0} KB", bytes as f64 / 1000.0)
    }
}

fn explain(b: &Breakdown, smallest: usize, limit: usize, floor: Option<Setting>) -> String {
    let text = b.fonts + b.content;
    if text >= limit {
        return format!(
            "The text and fonts alone are {}, more than the {} limit, and pdfit never changes those. \
             The smallest it got was {}.",
            mb(text),
            mb(limit),
            mb(smallest)
        );
    }
    if text + b.images_kept >= limit {
        return format!(
            "Images pdfit can't recompress ({}) and the text and fonts ({}) already add up to more than {}. \
             The smallest it got was {}.",
            mb(b.images_kept),
            mb(text),
            mb(limit),
            mb(smallest)
        );
    }
    let at = floor.map_or("after lossless clean-up".to_string(), |s| {
        format!(
            "at the lowest setting ({} dpi, JPEG quality {})",
            s.dpi, s.quality
        )
    });
    format!(
        "The smallest it got was {} {at}, {} over the limit. Images still take {} and text and fonts {}.",
        mb(smallest),
        mb(smallest - limit),
        mb(b.images + b.images_kept),
        mb(text)
    )
}

/// Sort stored stream bytes into images, fonts, content and the rest.
/// Object-stream packing and the cross-reference table aren't counted, so
/// the parts add up to a little less than the file.
pub fn breakdown(doc: &Document, kept: &HashSet<ObjectId>) -> Breakdown {
    let mut fonts = HashSet::new();
    let mut content = HashSet::new();
    for (&_id, obj) in &doc.objects {
        let d = match obj {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };
        match d.get(b"Type").and_then(Object::as_name) {
            Ok(b"FontDescriptor") => {
                for k in [b"FontFile".as_slice(), b"FontFile2", b"FontFile3"] {
                    if let Ok(r) = d.get(k).and_then(Object::as_reference) {
                        fonts.insert(r);
                    }
                }
            }
            Ok(b"Font") => {
                if let Ok(r) = d.get(b"ToUnicode").and_then(Object::as_reference) {
                    fonts.insert(r);
                }
            }
            Ok(b"Page") => match d.get(b"Contents") {
                Ok(Object::Reference(r)) => {
                    content.insert(*r);
                }
                Ok(Object::Array(a)) => {
                    content.extend(a.iter().filter_map(|o| o.as_reference().ok()))
                }
                _ => {}
            },
            _ => {}
        }
    }
    let mut b = Breakdown::default();
    for (id, obj) in &doc.objects {
        let Object::Stream(s) = obj else { continue };
        let n = s.content.len();
        let subtype = s.dict.get(b"Subtype").and_then(Object::as_name).ok();
        if kept.contains(id) {
            b.images_kept += n;
        } else if subtype == Some(b"Image") {
            b.images += n;
        } else if fonts.contains(id) {
            b.fonts += n;
        } else if content.contains(id) || subtype == Some(b"Form") {
            b.content += n;
        } else {
            b.other += n;
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testpdf::Builder;

    fn scan_pages(n: usize) -> Vec<u8> {
        let mut b = Builder::new();
        for _ in 0..n {
            // A 2400 x 3000 px "scan" drawn on 8 x 10 inches: 300 dpi.
            let img = b.jpeg_image(2400, 3000, 95);
            b.page("q 576 0 0 720 18 36 cm /Im0 Do Q", &[("Im0", img)], None);
        }
        b.bytes()
    }

    #[test]
    fn stops_at_the_first_rung_that_fits_and_not_lower() {
        let input = scan_pages(2);
        // Measure every rung, then ask for a limit just above one of them.
        let p = Prepared::new(doc::load(&input).unwrap());
        let sizes: Vec<usize> = LADDER
            .iter()
            .map(|&s| p.render(Some(s)).unwrap().bytes.len())
            .collect();
        assert!(
            sizes.windows(2).all(|w| w[0] >= w[1]),
            "sizes should fall: {sizes:?}"
        );
        let limit = sizes[4] + 1;
        let out = fit(&input, limit).unwrap();
        assert!(out.fits);
        assert_eq!(out.setting, Some(LADDER[4]));
        assert!(out.output_bytes <= limit);
        // Binary search: lossless, floor, then about log2(9) more.
        assert!(out.attempts.len() <= 6, "{:?}", out.attempts);
    }

    #[test]
    fn a_file_already_under_the_limit_comes_back_untouched() {
        let input = scan_pages(1);
        let out = fit(&input, input.len() + 10).unwrap();
        assert!(out.already_fits);
        assert_eq!(out.output, input);
    }

    #[test]
    fn when_fonts_alone_are_too_big_it_says_so() {
        let mut b = Builder::new();
        // An incompressible font program, standing in for a big CJK font.
        let mut x = 0x9e37_79b9_u32;
        let program: Vec<u8> = (0..600_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        let f = b.font(&program);
        b.page("BT /F1 12 Tf (hi) Tj ET", &[], Some(f));
        let input = b.bytes();
        let out = fit(&input, 400_000).unwrap();
        assert!(!out.fits);
        let reason = out.reason.unwrap();
        assert!(
            reason.starts_with("The text and fonts alone are 0.6 MB"),
            "{reason}"
        );
    }
}
