//! Apply one (dpi, quality) setting to every image that can take it, and
//! write the result.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use rayon::prelude::*;
use serde::Serialize;

use crate::doc::{self, Error, Image, Inventory};
use crate::raster::{self, Pixels};
use crate::recompress::{self, drawn_inches, target_size};
use crate::tidy::{self, Tidied};

/// A rung on the quality ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Setting {
    pub dpi: u32,
    pub quality: u8,
}

/// A new image is only kept if it saves at least this share; re-encoding a
/// JPEG for a 2% saving isn't worth the loss.
const MIN_SAVING: f64 = 0.05;

/// What happened to one image under one setting.
#[derive(Clone, Debug, Serialize)]
pub struct Change {
    pub obj: u32,
    pub before: usize,
    pub after: usize,
    pub width: u32,
    pub height: u32,
}

/// The document after lossless tidying, ready to have settings tried on
/// it. Pixels are cached between tries: two settings at the same dpi share
/// the resampled image and only the JPEG step runs again.
pub struct Prepared {
    pub doc: Document,
    pub inventory: Inventory,
    pub tidied: Tidied,
    cache: Mutex<HashMap<(ObjectId, u32, u32), Arc<Pixels>>>,
}

pub struct Rendered {
    pub bytes: Vec<u8>,
    pub doc: Document,
    pub changes: Vec<Change>,
}

struct Replacement {
    image: (ObjectId, Stream),
    smask: Option<(ObjectId, Stream)>,
    change: Change,
}

impl Prepared {
    pub fn new(mut doc: Document) -> Prepared {
        let tidied = tidy::tidy(&mut doc);
        let inventory = doc::inventory(&doc);
        Prepared {
            doc,
            inventory,
            tidied,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Write the document with `setting` applied, or with only the lossless
    /// tidying when it's None.
    pub fn render(&self, setting: Option<Setting>) -> Result<Rendered, Error> {
        let mut doc = self.doc.clone();
        let mut changes = Vec::new();
        if let Some(s) = setting {
            let done: Vec<Replacement> = self
                .inventory
                .images
                .par_iter()
                .filter(|i| i.info.skip.is_none())
                .filter_map(|i| self.recompress(i, s))
                .collect();
            for r in done {
                doc.objects.insert(r.image.0, Object::Stream(r.image.1));
                if let Some((id, s)) = r.smask {
                    doc.objects.insert(id, Object::Stream(s));
                }
                changes.push(r.change);
            }
        }
        let mut bytes = Vec::new();
        doc.save_modern(&mut bytes)
            .map_err(|e| Error::Write(e.to_string()))?;
        Ok(Rendered {
            bytes,
            doc,
            changes,
        })
    }

    fn pixels(&self, id: ObjectId, size: Option<(u32, u32)>) -> Option<Arc<Pixels>> {
        // The key uses 0 x 0 for "as stored" so the full-size decode can be
        // found without knowing its size.
        let key = (id, size.map_or(0, |s| s.0), size.map_or(0, |s| s.1));
        if let Some(p) = self.cache.lock().unwrap().get(&key) {
            return Some(p.clone());
        }
        let stream = self.doc.get_object(id).ok()?.as_stream().ok()?;
        let (full, _) = raster::decode(&self.doc, stream).ok()?;
        let px = match size {
            Some((w, h)) if (w, h) != (full.width, full.height) => recompress::resize(&full, w, h),
            _ => full,
        };
        let px = Arc::new(px);
        self.cache.lock().unwrap().insert(key, px.clone());
        Some(px)
    }

    /// An image and its soft mask, resized together. When both are the
    /// same size and the mask has no /Matte, the colour is resized
    /// premultiplied by the mask, so fully transparent pixels (often black)
    /// don't bleed into the edges of what's visible; a blue signature
    /// otherwise comes out grey.
    fn masked_pixels(
        &self,
        img: ObjectId,
        mask: ObjectId,
        size: (u32, u32),
        matte: bool,
    ) -> Option<(Arc<Pixels>, Arc<Pixels>)> {
        let key = |id| (id, size.0, size.1);
        {
            let c = self.cache.lock().unwrap();
            if let (Some(p), Some(m)) = (c.get(&key(img)), c.get(&key(mask))) {
                return Some((p.clone(), m.clone()));
            }
        }
        let full = self.pixels(img, None)?;
        let full_mask = self.pixels(mask, None)?;
        if full_mask.channels != 1 {
            return None;
        }
        let same_size = (full.width, full.height) == (full_mask.width, full_mask.height);
        let (px, m) = if same_size && !matte {
            recompress::resize_with_alpha(&full, &full_mask, size.0, size.1)
        } else {
            (
                recompress::resize(&full, size.0, size.1),
                recompress::resize(&full_mask, size.0, size.1),
            )
        };
        let (px, m) = (Arc::new(px), Arc::new(m));
        let mut c = self.cache.lock().unwrap();
        c.insert(key(img), px.clone());
        c.insert(key(mask), m.clone());
        Some((px, m))
    }

    fn recompress(&self, img: &Image, s: Setting) -> Option<Replacement> {
        let old = self.doc.get_object(img.id).ok()?.as_stream().ok()?;
        let (w, h) = (img.info.width, img.info.height);
        // Images never seen being drawn (patterns, unusual annotations)
        // keep their pixel size; only the JPEG quality changes.
        let target =
            drawn_inches(&img.placements).and_then(|inches| target_size(w, h, inches, s.dpi));
        let old_mask = img
            .smask
            .and_then(|m| Some((m, self.doc.get_object(m).ok()?.as_stream().ok()?)));
        let mut smask = None;
        let px = match (old_mask, target) {
            (Some((mask_id, mask)), Some((nw, nh))) => {
                // A soft mask can have its own resolution, except when it
                // has a /Matte entry: then it must match the image exactly.
                match self.masked_pixels(img.id, mask_id, (nw, nh), mask.dict.has(b"Matte")) {
                    Some((px, m)) => {
                        smask = Some((mask_id, mask_stream(&mask.dict, &m)));
                        px
                    }
                    None if mask.dict.has(b"Matte") => return None,
                    None => self.pixels(img.id, target)?,
                }
            }
            _ => self.pixels(img.id, target)?,
        };
        let jpg = recompress::jpeg(&px, s.quality);
        if jpg.is_empty() {
            return None;
        }

        let before = old.content.len() + old_mask.map_or(0, |(_, m)| m.content.len());
        let after = jpg.len()
            + match (&smask, old_mask) {
                (Some((_, s)), _) => s.content.len(),
                (None, Some((_, m))) => m.content.len(),
                (None, None) => 0,
            };
        if after as f64 > before as f64 * (1.0 - MIN_SAVING) {
            return None;
        }

        let mut dict = old.dict.clone();
        for k in [b"Filter".as_slice(), b"DecodeParms", b"DP", b"Length"] {
            dict.remove(k);
        }
        dict.set("Width", px.width as i64);
        dict.set("Height", px.height as i64);
        dict.set("BitsPerComponent", 8);
        dict.set("Filter", "DCTDecode");
        Some(Replacement {
            image: (img.id, Stream::new(dict, jpg).with_compression(false)),
            smask,
            change: Change {
                obj: img.id.0,
                before,
                after,
                width: px.width,
                height: px.height,
            },
        })
    }
}

/// Soft masks are stored losslessly: JPEG ringing around a cut-out edge
/// shows as a halo.
fn mask_stream(old: &Dictionary, m: &Pixels) -> Stream {
    let mut dict = old.clone();
    for k in [
        b"Filter".as_slice(),
        b"DecodeParms",
        b"DP",
        b"Length",
        b"Decode",
    ] {
        dict.remove(k);
    }
    dict.set("Width", m.width as i64);
    dict.set("Height", m.height as i64);
    dict.set("BitsPerComponent", 8);
    dict.set("ColorSpace", "DeviceGray");
    dict.set("Filter", "FlateDecode");
    Stream::new(dict, recompress::deflate(&m.data)).with_compression(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testpdf::Builder;

    fn decode(doc: &Document, id: ObjectId) -> Pixels {
        raster::decode(doc, doc.get_object(id).unwrap().as_stream().unwrap())
            .unwrap()
            .0
    }

    #[test]
    fn an_oversized_scan_is_resampled_to_the_target_dpi() {
        let mut b = Builder::new();
        // 1600 x 2000 px drawn 4 x 5 inches: 400 dpi.
        let img = b.jpeg_image(1600, 2000, 95);
        b.page("q 288 0 0 360 0 0 cm /Im0 Do Q", &[("Im0", img)], None);
        let p = Prepared::new(b.finish());
        let out = p
            .render(Some(Setting {
                dpi: 150,
                quality: 75,
            }))
            .unwrap();
        assert_eq!(out.changes.len(), 1);
        let reread = doc::load(&out.bytes).unwrap();
        let px = decode(&reread, img);
        assert_eq!((px.width, px.height), (600, 750));
        assert!(out.changes[0].after < out.changes[0].before / 4);
    }

    #[test]
    fn soft_masks_follow_the_image_size() {
        let mut b = Builder::new();
        let img = b.jpeg_image(1200, 1200, 95);
        let mask = b.flate_image(1200, 1200, "DeviceGray", 1);
        b.doc
            .get_object_mut(img)
            .unwrap()
            .as_stream_mut()
            .unwrap()
            .dict
            .set("SMask", mask);
        b.page("q 144 0 0 144 0 0 cm /Im0 Do Q", &[("Im0", img)], None);
        let p = Prepared::new(b.finish());
        let out = p
            .render(Some(Setting {
                dpi: 150,
                quality: 75,
            }))
            .unwrap();
        let reread = doc::load(&out.bytes).unwrap();
        let m = decode(&reread, mask);
        assert_eq!((m.width, m.height, m.channels), (300, 300, 1));
        let i = reread.get_object(img).unwrap().as_stream().unwrap();
        assert_eq!(i.dict.get(b"SMask").unwrap().as_reference().unwrap(), mask);
    }

    #[test]
    fn a_bigger_result_is_thrown_away() {
        let mut b = Builder::new();
        // Already a low-quality JPEG at its drawn size: nothing to gain.
        let img = b.jpeg_image(300, 300, 30);
        b.page("q 144 0 0 144 0 0 cm /Im0 Do Q", &[("Im0", img)], None);
        let p = Prepared::new(b.finish());
        let out = p
            .render(Some(Setting {
                dpi: 150,
                quality: 85,
            }))
            .unwrap();
        assert!(out.changes.is_empty());
    }

    #[test]
    fn shared_images_are_rewritten_once_for_the_largest_use() {
        let mut b = Builder::new();
        let img = b.jpeg_image(2000, 1000, 95);
        b.page("q 72 0 0 36 0 0 cm /Im0 Do Q", &[("Im0", img)], None);
        b.page("q 576 0 0 288 0 0 cm /Im0 Do Q", &[("Im0", img)], None);
        let p = Prepared::new(b.finish());
        let out = p
            .render(Some(Setting {
                dpi: 100,
                quality: 75,
            }))
            .unwrap();
        assert_eq!(out.changes.len(), 1);
        // Largest use is 8 x 4 inches; 100 dpi needs 800 x 400.
        assert_eq!((out.changes[0].width, out.changes[0].height), (800, 400));
    }
}
