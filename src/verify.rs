//! Check that the text and drawing of a document came through unchanged.
//!
//! For every page this collects the decoded content stream, the decoded
//! content of each form it uses, and each font: its dictionary entries that
//! affect rendering and the decoded font program. Two documents with equal
//! fingerprints draw the same text with the same fonts in the same places.
//! Only image data is allowed to differ.

use std::collections::BTreeMap;

use lopdf::{Dictionary, Document, Object};

const MAX_DEPTH: usize = 12;

/// Page number to everything on that page except image pixels.
pub type Fingerprint = BTreeMap<u32, Vec<(String, Vec<u8>)>>;

pub fn fingerprint(doc: &Document) -> Fingerprint {
    let mut out = BTreeMap::new();
    for (n, id) in doc.get_pages() {
        let mut parts = vec![("content".to_string(), doc.get_page_content(id))];
        if let Ok(page) = doc.get_dictionary(id)
            && let Some(res) = resources(doc, page)
        {
            collect(doc, &res, "", &mut parts, 0);
        }
        out.insert(n, parts);
    }
    out
}

fn resources(doc: &Document, page: &Dictionary) -> Option<Dictionary> {
    let mut node = page;
    for _ in 0..MAX_DEPTH {
        if let Ok(r) = node.get(b"Resources")
            && let Ok((_, Object::Dictionary(d))) = doc.dereference(r)
        {
            return Some(d.clone());
        }
        node = doc
            .get_dictionary(node.get(b"Parent").ok()?.as_reference().ok()?)
            .ok()?;
    }
    None
}

fn sub_dict<'a>(doc: &'a Document, d: &'a Dictionary, key: &[u8]) -> Option<&'a Dictionary> {
    doc.dereference(d.get(key).ok()?).ok()?.1.as_dict().ok()
}

fn decoded(doc: &Document, o: &Object) -> Vec<u8> {
    match doc.dereference(o) {
        Ok((_, Object::Stream(s))) => s.get_plain_content().unwrap_or_else(|_| s.content.clone()),
        _ => Vec::new(),
    }
}

fn collect(
    doc: &Document,
    res: &Dictionary,
    path: &str,
    parts: &mut Vec<(String, Vec<u8>)>,
    depth: usize,
) {
    if depth > MAX_DEPTH {
        return;
    }
    if let Some(fonts) = sub_dict(doc, res, b"Font") {
        for (name, f) in fonts.iter() {
            let Ok((_, Object::Dictionary(font))) = doc.dereference(f) else {
                continue;
            };
            let key = format!("{path}font {}", String::from_utf8_lossy(name));
            parts.push((key.clone(), font_bytes(doc, font)));
        }
    }
    if let Some(xobjects) = sub_dict(doc, res, b"XObject") {
        for (name, x) in xobjects.iter() {
            let Ok((_, Object::Stream(s))) = doc.dereference(x) else {
                continue;
            };
            if !matches!(
                s.dict.get(b"Subtype").and_then(Object::as_name),
                Ok(b"Form")
            ) {
                continue;
            }
            let key = format!("{path}form {}/", String::from_utf8_lossy(name));
            parts.push((
                key.clone(),
                s.get_plain_content().unwrap_or_else(|_| s.content.clone()),
            ));
            if let Some(inner) = sub_dict(doc, &s.dict, b"Resources") {
                collect(doc, inner, &key, parts, depth + 1);
            }
        }
    }
}

/// The parts of a font that change what's drawn. Object numbers are left
/// out on purpose: merging duplicate streams renumbers them harmlessly.
fn font_bytes(doc: &Document, font: &Dictionary) -> Vec<u8> {
    let mut v = Vec::new();
    for key in [
        b"Subtype".as_slice(),
        b"BaseFont",
        b"Encoding",
        b"FirstChar",
        b"LastChar",
    ] {
        if let Ok(o) = font.get(key) {
            v.extend(format!("{:?}", doc.dereference(o).map(|x| x.1).unwrap_or(o)).bytes());
        }
    }
    if let Ok(w) = font.get(b"Widths") {
        v.extend(format!("{:?}", doc.dereference(w).map(|x| x.1).unwrap_or(w)).bytes());
    }
    if let Ok(t) = font.get(b"ToUnicode") {
        v.extend(decoded(doc, t));
    }
    let descriptor = sub_dict(doc, font, b"FontDescriptor").or_else(|| {
        // Type0 fonts keep the program in their descendant font.
        let d = doc
            .dereference(font.get(b"DescendantFonts").ok()?)
            .ok()?
            .1
            .as_array()
            .ok()?;
        let first = doc.dereference(d.first()?).ok()?.1.as_dict().ok()?;
        sub_dict(doc, first, b"FontDescriptor")
    });
    if let Some(fd) = descriptor {
        for key in [b"FontFile".as_slice(), b"FontFile2", b"FontFile3"] {
            if let Ok(f) = fd.get(key) {
                v.extend(decoded(doc, f));
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fit;
    use crate::testpdf::Builder;

    /// Text, a font program, a vector drawing inside a form and a big
    /// photo: everything but the photo must come out byte for byte.
    #[test]
    fn content_streams_and_fonts_are_byte_identical_after_shrinking() {
        let mut b = Builder::new();
        let program: Vec<u8> = (0..40_000u32).map(|i| (i % 253) as u8).collect();
        let font = b.font(&program);
        let photo = b.jpeg_image(2400, 1800, 95);
        let chart = b.form("0 0 m 100 100 l S 1 0 0 RG 10 10 80 40 re f", &[], None);
        let text = "BT /F1 11 Tf 72 700 Td (Name: A. Sample   ID 4471-0093) Tj ET\n";
        b.page(
            &format!("{text}q 432 0 0 324 72 300 cm /Im0 Do Q q /Fm0 Do Q"),
            &[("Im0", photo), ("Fm0", chart)],
            Some(font),
        );
        let input = b.bytes();
        let original = crate::doc::load(&input).unwrap();

        let out = fit::fit(&input, 200_000).unwrap();
        assert!(out.fits && !out.images.is_empty());
        assert!(
            out.images[0].after.is_some(),
            "the photo should have been recompressed"
        );
        let result = crate::doc::load(&out.output).unwrap();
        assert_eq!(fingerprint(&original), fingerprint(&result));

        // And directly, without the fingerprint's help.
        let page = |d: &Document| d.get_page_content(*d.get_pages().get(&1).unwrap());
        assert_eq!(page(&original), page(&result));
        assert_eq!(
            page(&result),
            format!("{text}q 432 0 0 324 72 300 cm /Im0 Do Q q /Fm0 Do Q\n").as_bytes()
        );
        let fp = fingerprint(&result);
        let font_part = fp[&1].iter().find(|(k, _)| k == "font F1").unwrap();
        assert!(font_part.1.ends_with(&program));
    }

    #[test]
    fn a_changed_letter_changes_the_fingerprint() {
        let make = |s: &str| {
            let mut b = Builder::new();
            b.page(s, &[], None);
            fingerprint(&b.finish())
        };
        assert_ne!(make("BT (ID 4471) Tj ET"), make("BT (ID 4477) Tj ET"));
    }
}
