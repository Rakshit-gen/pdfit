//! Savings that cost nothing in quality: page thumbnails nobody's viewer
//! shows any more, streams stored twice, objects nothing points to, and
//! streams saved without compression.

use std::collections::HashMap;

use lopdf::{Document, Object, ObjectId};
use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Tidied {
    pub thumbnails: usize,
    pub duplicates: usize,
    pub unused: usize,
    pub compressed: usize,
}

pub fn tidy(doc: &mut Document) -> Tidied {
    let thumbnails = drop_thumbnails(doc);
    let duplicates = merge_duplicate_streams(doc);
    let unused = doc.prune_objects().len();
    let compressed = compress_plain_streams(doc);
    Tidied {
        thumbnails,
        duplicates,
        unused,
        compressed,
    }
}

fn drop_thumbnails(doc: &mut Document) -> usize {
    let mut n = 0;
    for page in doc.get_pages().into_values() {
        if let Ok(d) = doc.get_dictionary_mut(page)
            && d.remove(b"Thumb").is_some()
        {
            n += 1;
        }
    }
    n
}

/// Byte-identical streams (same dictionary, same data) become one object.
/// Common in merged files, where every page brought its own copy of the
/// letterhead logo and the same fonts.
fn merge_duplicate_streams(doc: &mut Document) -> usize {
    let mut first: HashMap<Vec<u8>, ObjectId> = HashMap::new();
    let mut remap: HashMap<ObjectId, ObjectId> = HashMap::new();
    for (&id, obj) in &doc.objects {
        let Object::Stream(s) = obj else { continue };
        // Tiny streams aren't worth the hashing, and the key holds the data.
        if s.content.len() < 256 {
            continue;
        }
        let mut dict = s.dict.clone();
        dict.remove(b"Length");
        // lopdf doesn't export its serializer; Debug output is just as
        // deterministic for comparing two dictionaries.
        let mut key = format!("{dict:?}").into_bytes();
        key.extend_from_slice(&s.content);
        match first.get(&key) {
            Some(&keep) => {
                remap.insert(id, keep);
            }
            None => {
                first.insert(key, id);
            }
        }
    }
    if remap.is_empty() {
        return 0;
    }
    for obj in doc.objects.values_mut() {
        repoint(obj, &remap);
    }
    for v in doc.trailer.iter_mut().map(|(_, v)| v) {
        repoint(v, &remap);
    }
    for id in remap.keys() {
        doc.objects.remove(id);
    }
    remap.len()
}

fn repoint(obj: &mut Object, remap: &HashMap<ObjectId, ObjectId>) {
    match obj {
        Object::Reference(id) => {
            if let Some(&to) = remap.get(id) {
                *id = to;
            }
        }
        Object::Array(a) => a.iter_mut().for_each(|o| repoint(o, remap)),
        Object::Dictionary(d) => d.iter_mut().for_each(|(_, o)| repoint(o, remap)),
        Object::Stream(s) => s.dict.iter_mut().for_each(|(_, o)| repoint(o, remap)),
        _ => {}
    }
}

/// Flate-compress streams stored raw. XMP metadata is skipped: it's meant
/// to stay readable by tools that scan files for it without a PDF parser.
fn compress_plain_streams(doc: &mut Document) -> usize {
    let mut n = 0;
    for obj in doc.objects.values_mut() {
        let Object::Stream(s) = obj else { continue };
        if s.dict.has(b"Filter")
            || matches!(
                s.dict.get(b"Type").and_then(Object::as_name),
                Ok(b"Metadata")
            )
        {
            continue;
        }
        let before = s.content.len();
        let _ = s.compress();
        if s.content.len() < before {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testpdf::Builder;
    use lopdf::{Stream, dictionary};

    #[test]
    fn duplicate_images_become_one_and_both_pages_point_at_it() {
        let mut b = Builder::new();
        let a = b.jpeg_image(64, 64, 80);
        let copy = b.jpeg_image(64, 64, 80);
        let p1 = b.page("/X Do", &[("X", a)], None);
        let p2 = b.page("/X Do", &[("X", copy)], None);
        let mut doc = b.finish();
        let t = tidy(&mut doc);
        assert_eq!(t.duplicates, 1);
        assert!(doc.objects.contains_key(&a) && !doc.objects.contains_key(&copy));
        for p in [p1, p2] {
            let res = doc
                .get_dictionary(p)
                .unwrap()
                .get(b"Resources")
                .unwrap()
                .as_dict()
                .unwrap();
            let x = res.get(b"XObject").unwrap().as_dict().unwrap();
            assert_eq!(x.get(b"X").unwrap().as_reference().unwrap(), a);
        }
    }

    #[test]
    fn thumbnails_and_orphans_go() {
        let mut b = Builder::new();
        let thumb = b.flate_image(60, 80, "DeviceRGB", 3);
        let orphan = b.jpeg_image(30, 30, 80);
        let page = b.page("", &[], None);
        let mut doc = b.finish();
        doc.get_dictionary_mut(page).unwrap().set("Thumb", thumb);
        let t = tidy(&mut doc);
        assert_eq!(t.thumbnails, 1);
        assert!(t.unused >= 2);
        assert!(!doc.objects.contains_key(&thumb) && !doc.objects.contains_key(&orphan));
    }

    #[test]
    fn raw_streams_get_compressed_but_decode_the_same() {
        let mut b = Builder::new();
        let text =
            "BT /F1 12 Tf 72 720 Td (Hello, the same words again and again) Tj ET\n".repeat(40);
        let page = b.page(&text, &[], None);
        let meta = b.doc.add_object(Stream::new(
            dictionary! { "Type" => "Metadata", "Subtype" => "XML" },
            b"<x:xmpmeta>".repeat(100),
        ));
        let mut doc = b.finish();
        doc.catalog_mut().unwrap().set("Metadata", meta);
        tidy(&mut doc);
        let content_id = doc.get_page_contents(page)[0];
        let s = doc.get_object(content_id).unwrap().as_stream().unwrap();
        assert!(s.dict.has(b"Filter") && s.content.len() < text.len());
        assert_eq!(s.decompressed_content().unwrap(), text.as_bytes());
        let m = doc.get_object(meta).unwrap().as_stream().unwrap();
        assert!(!m.dict.has(b"Filter"));
    }
}
