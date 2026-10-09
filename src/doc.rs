//! Loading a PDF and listing what's heavy in it.

use std::collections::HashSet;

use lopdf::{Document, Object, ObjectId};
use serde::Serialize;

use crate::raster::{self, Model};
use crate::recompress::{drawn_inches, effective_dpi};
use crate::scan;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "This PDF is encrypted or password-protected. pdfit won't rewrite it, because the copy would lose that \
         protection. Save an unprotected copy from the app that made it (or with the password, in a PDF reader), \
         then try again."
    )]
    Encrypted,
    #[error("This doesn't look like a PDF ({0}). Check that you picked the right file.")]
    NotPdf(String),
    #[error(
        "The PDF is damaged and couldn't be read ({0}). Opening and re-saving it in a PDF reader often repairs it."
    )]
    Broken(String),
    #[error("Couldn't write the smaller PDF: {0}")]
    Write(String),
}

impl Error {
    /// A short name for the page to pick its message and state by.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Encrypted => "encrypted",
            Error::NotPdf(_) => "not_pdf",
            Error::Broken(_) => "broken",
            Error::Write(_) => "write",
        }
    }
}

pub fn load(bytes: &[u8]) -> Result<Document, Error> {
    let head = &bytes[..bytes.len().min(1024)];
    if !head.windows(5).any(|w| w == b"%PDF-") {
        return Err(Error::NotPdf("no PDF header at the start".into()));
    }
    let doc = match Document::load_mem(bytes) {
        Ok(d) => d,
        // With a user password lopdf can't open it at all; say why rather
        // than reporting it as damaged.
        Err(_) if has_encrypt_entry(bytes) => return Err(Error::Encrypted),
        Err(e) => return Err(Error::Broken(e.to_string())),
    };
    // Files with only an owner password open without asking, and lopdf
    // decrypts them on load. Rewriting would silently drop the
    // restrictions, so they're refused too.
    if doc.trailer.has(b"Encrypt") || doc.is_encrypted() || doc.was_encrypted() {
        return Err(Error::Encrypted);
    }
    if doc.get_pages().is_empty() {
        return Err(Error::Broken("it has no pages".into()));
    }
    Ok(doc)
}

fn has_encrypt_entry(bytes: &[u8]) -> bool {
    bytes.windows(8).any(|w| w == b"/Encrypt")
}

/// One image XObject as the person sees it in the list.
#[derive(Clone, Debug, Serialize)]
pub struct ImageInfo {
    /// PDF object number, to tell images apart.
    pub obj: u32,
    pub pages: Vec<u32>,
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub color: String,
    /// Stored size, including the soft mask if it has one.
    pub bytes: usize,
    /// Largest size it is drawn at, in inches.
    pub drawn: Option<(f64, f64)>,
    pub dpi: Option<f64>,
    pub has_mask: bool,
    /// Why pdfit won't recompress it, if it won't.
    pub skip: Option<String>,
}

/// Images smaller than this aren't worth a generation of JPEG loss.
pub const SMALL_IMAGE: usize = 16 * 1024;

#[derive(Debug)]
pub struct Image {
    pub id: ObjectId,
    pub smask: Option<ObjectId>,
    pub model: Option<Model>,
    pub placements: Vec<scan::Placement>,
    pub info: ImageInfo,
}

#[derive(Debug, Default, Serialize)]
pub struct Inventory {
    pub pages: usize,
    pub inline_images: usize,
    pub thumbnails: usize,
    pub unreadable_streams: usize,
    #[serde(skip)]
    pub images: Vec<Image>,
}

pub fn stream_len(doc: &Document, id: ObjectId) -> usize {
    match doc.get_object(id) {
        Ok(Object::Stream(s)) => s.content.len(),
        _ => 0,
    }
}

fn int(o: Result<&Object, lopdf::Error>) -> u32 {
    o.ok().and_then(|o| o.as_i64().ok()).unwrap_or(0).max(0) as u32
}

pub fn thumbnails(doc: &Document) -> Vec<ObjectId> {
    doc.get_pages()
        .values()
        .filter_map(|&p| {
            doc.get_dictionary(p)
                .ok()?
                .get(b"Thumb")
                .ok()?
                .as_reference()
                .ok()
        })
        .collect()
}

fn format_name(doc: &Document, dict: &lopdf::Dictionary) -> String {
    let f = raster::filters(doc, dict);
    match f.last().map(Vec::as_slice) {
        None => "Uncompressed".into(),
        Some(b"DCTDecode" | b"DCT") => "JPEG".into(),
        Some(b"FlateDecode" | b"Fl") => "Flate".into(),
        Some(b"JPXDecode") => "JPEG 2000".into(),
        Some(b"JBIG2Decode") => "JBIG2".into(),
        Some(b"CCITTFaxDecode" | b"CCF") => "CCITT".into(),
        Some(other) => String::from_utf8_lossy(other)
            .trim_end_matches("Decode")
            .to_string(),
    }
}

/// Every image XObject that's drawn as an image in its own right, with
/// where it's drawn and whether pdfit can work on it. Soft masks belong to
/// their image and page thumbnails are dropped, so neither is listed.
pub fn inventory(doc: &Document) -> Inventory {
    let found = scan::find_images(doc);
    let thumbs: HashSet<ObjectId> = thumbnails(doc).into_iter().collect();
    let mut masks = HashSet::new();
    for obj in doc.objects.values() {
        if let Object::Stream(s) = obj
            && matches!(
                s.dict.get(b"Subtype").and_then(Object::as_name),
                Ok(b"Image")
            )
        {
            for key in [b"SMask".as_slice(), b"Mask"] {
                if let Ok(id) = s.dict.get(key).and_then(Object::as_reference) {
                    masks.insert(id);
                }
            }
        }
    }

    let mut images = Vec::new();
    for (&id, obj) in &doc.objects {
        let Object::Stream(s) = obj else { continue };
        if !matches!(
            s.dict.get(b"Subtype").and_then(Object::as_name),
            Ok(b"Image")
        ) || masks.contains(&id)
            || thumbs.contains(&id)
        {
            continue;
        }
        let d = &s.dict;
        let smask = d.get(b"SMask").and_then(Object::as_reference).ok();
        let placements = found.placements.get(&id).cloned().unwrap_or_default();
        let (width, height) = (int(d.get(b"Width")), int(d.get(b"Height")));
        let drawn = drawn_inches(&placements);
        let classified = raster::classify(doc, d);
        let bytes = s.content.len() + smask.map_or(0, |m| stream_len(doc, m));
        let mut pages: Vec<u32> = placements.iter().map(|p| p.page).collect();
        pages.dedup();
        let skip = match &classified {
            Err(why) => Some(why.to_string()),
            Ok(_) if bytes < SMALL_IMAGE => Some("small already, not worth touching".into()),
            Ok(_) => None,
        };
        let color = match classified {
            Ok(Model::Gray) => "Gray",
            Ok(Model::Rgb) => "RGB",
            Ok(Model::Cmyk) => "CMYK",
            Err(raster::Skip::Palette) => "Palette",
            Err(_) if int(d.get(b"BitsPerComponent")) == 1 || d.has(b"ImageMask") => "1-bit",
            Err(_) => "Other",
        };
        images.push(Image {
            id,
            smask,
            model: classified.ok(),
            info: ImageInfo {
                obj: id.0,
                pages,
                width,
                height,
                format: format_name(doc, d),
                color: color.into(),
                bytes,
                dpi: drawn.map(|i| effective_dpi(width.max(1), height.max(1), i)),
                drawn,
                has_mask: smask.is_some(),
                skip,
            },
            placements,
        });
    }
    images.sort_by_key(|i| {
        (
            std::cmp::Reverse(i.info.bytes),
            i.info.pages.first().copied(),
        )
    });
    Inventory {
        pages: doc.get_pages().len(),
        inline_images: found.inline_images,
        thumbnails: thumbs.len(),
        unreadable_streams: found.unreadable_streams,
        images,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testpdf::Builder;

    #[test]
    fn not_a_pdf_is_refused_with_a_reason() {
        assert!(matches!(load(b"hello"), Err(Error::NotPdf(_))));
    }

    #[test]
    fn owner_password_only_files_are_refused() {
        // lopdf can write an encrypted file; an empty user password means
        // it opens without asking, the case most likely to slip through.
        let mut b = Builder::new();
        b.page("BT ET", &[], None);
        let mut doc = b.finish();
        doc.trailer.set(
            "ID",
            vec![
                Object::string_literal(vec![1u8; 16]),
                Object::string_literal(vec![1u8; 16]),
            ],
        );
        let state = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V2 {
            document: &doc,
            owner_password: "owner",
            user_password: "",
            key_length: 128,
            permissions: lopdf::Permissions::empty(),
        })
        .unwrap();
        doc.encrypt(&state).unwrap();
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        assert!(matches!(load(&bytes), Err(Error::Encrypted)));
    }

    #[test]
    fn inventory_lists_drawn_images_biggest_first_without_masks() {
        let mut b = Builder::new();
        let small = b.jpeg_image(40, 40, 80);
        let big = b.jpeg_image(800, 600, 92);
        let mask = b.flate_image(800, 600, "DeviceGray", 1);
        b.doc
            .get_object_mut(big)
            .unwrap()
            .as_stream_mut()
            .unwrap()
            .dict
            .set("SMask", mask);
        b.page(
            "q 288 0 0 216 0 0 cm /A Do Q q 36 0 0 36 0 0 cm /B Do Q",
            &[("A", big), ("B", small)],
            None,
        );
        let doc = b.finish();
        let inv = inventory(&doc);
        let objs: Vec<u32> = inv.images.iter().map(|i| i.info.obj).collect();
        assert_eq!(objs, vec![big.0, small.0]);
        let first = &inv.images[0].info;
        assert!(first.has_mask);
        assert_eq!(first.dpi.map(f64::round), Some(200.0));
        assert_eq!(first.skip, None);
        assert_eq!(
            inv.images[1].info.skip.as_deref(),
            Some("small already, not worth touching")
        );
    }
}
