//! Find out where each image is drawn and how big it ends up on the page.
//!
//! An image XObject has a pixel size but no physical size; that comes from
//! the current transformation matrix at the moment `Do` paints it. So this
//! runs a tiny interpreter over every content stream that only cares about
//! `q`, `Q`, `cm` and `Do`, and follows `Do` into Form XObjects (and into
//! annotation appearances) with their own matrices and resources.

use std::collections::BTreeMap;

use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::matrix::{Matrix, number};

/// Forms nest; real files rarely go past three or four levels. The limit is
/// for files that loop.
const MAX_DEPTH: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// 1-based page number.
    pub page: u32,
    /// Drawn size on the page, in points (1/72 inch).
    pub width_pt: f64,
    pub height_pt: f64,
}

#[derive(Debug, Default)]
pub struct Found {
    /// Every place each image XObject is painted.
    pub placements: BTreeMap<ObjectId, Vec<Placement>>,
    /// Inline images (`BI ... ID ... EI`) live inside content streams, which
    /// pdfit never rewrites, so they are only counted.
    pub inline_images: usize,
    /// Content streams that couldn't be parsed. Images drawn only from
    /// these are treated as having an unknown size.
    pub unreadable_streams: usize,
}

pub fn find_images(doc: &Document) -> Found {
    let mut found = Found::default();
    for (page_no, page_id) in doc.get_pages() {
        let Ok(page) = doc.get_dictionary(page_id) else {
            continue;
        };
        let resources = page_resources(doc, page).unwrap_or_default();
        let content = doc.get_page_content(page_id);
        let mut w = Walker {
            doc,
            page: page_no,
            found: &mut found,
            stack: Vec::new(),
        };
        w.run(&content, &resources, Matrix::IDENTITY, 0);
        w.annotations(page);
    }
    found
}

/// A page's resources, which may be inherited from an ancestor in the page
/// tree.
fn page_resources(doc: &Document, page: &Dictionary) -> Option<Dictionary> {
    let mut node = page;
    for _ in 0..MAX_DEPTH {
        if let Ok(r) = node.get(b"Resources")
            && let Ok((_, Object::Dictionary(d))) = doc.dereference(r)
        {
            return Some(d.clone());
        }
        let parent = node.get(b"Parent").ok()?.as_reference().ok()?;
        node = doc.get_dictionary(parent).ok()?;
    }
    None
}

struct Walker<'a> {
    doc: &'a Document,
    page: u32,
    found: &'a mut Found,
    /// Forms currently being walked, to stop a form that draws itself.
    stack: Vec<ObjectId>,
}

impl Walker<'_> {
    fn run(&mut self, content: &[u8], resources: &Dictionary, base: Matrix, depth: usize) {
        let Ok(ops) = Content::decode(content) else {
            self.found.unreadable_streams += 1;
            return;
        };
        let mut ctm = base;
        let mut saved = Vec::new();
        for op in &ops.operations {
            match op.operator.as_str() {
                "q" => saved.push(ctm),
                // An unbalanced Q is a file bug; readers ignore it, and so do we.
                "Q" => ctm = saved.pop().unwrap_or(base),
                "cm" => {
                    if let Some(m) = Matrix::from_operands(&op.operands) {
                        ctm = m.then(ctm);
                    }
                }
                "Do" => {
                    if let Some(Ok(name)) = op.operands.first().map(Object::as_name) {
                        self.paint(name, resources, ctm, depth);
                    }
                }
                "BI" => self.found.inline_images += 1,
                _ => {}
            }
        }
    }

    fn paint(&mut self, name: &[u8], resources: &Dictionary, ctm: Matrix, depth: usize) {
        let Some(id) = xobject_ref(self.doc, resources, name) else {
            return;
        };
        let Ok(Object::Stream(s)) = self.doc.get_object(id) else {
            return;
        };
        match s.dict.get(b"Subtype").and_then(Object::as_name) {
            Ok(b"Image") => {
                let (w, h) = ctm.unit_square_size();
                self.found
                    .placements
                    .entry(id)
                    .or_default()
                    .push(Placement {
                        page: self.page,
                        width_pt: w,
                        height_pt: h,
                    });
            }
            Ok(b"Form") => self.form(id, &s.dict, resources, ctm, depth),
            _ => {}
        }
    }

    fn form(
        &mut self,
        id: ObjectId,
        dict: &Dictionary,
        parent_resources: &Dictionary,
        ctm: Matrix,
        depth: usize,
    ) {
        if depth >= MAX_DEPTH || self.stack.contains(&id) {
            return;
        }
        let Ok(Object::Stream(s)) = self.doc.get_object(id) else {
            return;
        };
        let Ok(content) = s.get_plain_content() else {
            self.found.unreadable_streams += 1;
            return;
        };
        let matrix = dict
            .get(b"Matrix")
            .ok()
            .and_then(|m| m.as_array().ok())
            .and_then(|a| Matrix::from_operands(a))
            .unwrap_or(Matrix::IDENTITY);
        // A form without its own resources uses the ones of whatever drew it
        // (allowed in old files, still common).
        let resources = dict
            .get(b"Resources")
            .ok()
            .and_then(|r| self.doc.dereference(r).ok())
            .and_then(|(_, o)| o.as_dict().ok().cloned())
            .unwrap_or_else(|| parent_resources.clone());
        self.stack.push(id);
        self.run(&content, &resources, matrix.then(ctm), depth + 1);
        self.stack.pop();
    }

    /// Annotation appearances (signature fields, stamps) are forms drawn
    /// outside the page content, scaled so their box fills the annotation's
    /// rectangle.
    fn annotations(&mut self, page: &Dictionary) {
        let Ok(annots) = page.get(b"Annots").and_then(|a| self.doc.dereference(a)) else {
            return;
        };
        let Ok(annots) = annots.1.as_array() else {
            return;
        };
        for a in annots.clone() {
            let Ok((_, Object::Dictionary(annot))) = self.doc.dereference(&a) else {
                continue;
            };
            let Some(rect) = rect(self.doc, annot, b"Rect") else {
                continue;
            };
            let Some(ap) = annot
                .get(b"AP")
                .and_then(|ap| self.doc.dereference(ap))
                .ok()
                .and_then(|(_, o)| o.as_dict().ok())
            else {
                continue;
            };
            // Normal appearance: a stream, or a dict of states (checkboxes).
            let mut forms = Vec::new();
            match ap.get(b"N") {
                Ok(Object::Reference(id)) => match self.doc.get_object(*id) {
                    Ok(Object::Stream(_)) => forms.push(*id),
                    Ok(Object::Dictionary(states)) => {
                        forms.extend(states.iter().filter_map(|(_, v)| v.as_reference().ok()))
                    }
                    _ => {}
                },
                Ok(Object::Dictionary(states)) => {
                    forms.extend(states.iter().filter_map(|(_, v)| v.as_reference().ok()))
                }
                _ => {}
            }
            for id in forms {
                let Ok(Object::Stream(s)) = self.doc.get_object(id) else {
                    continue;
                };
                let dict = s.dict.clone();
                let Some(fit) = appearance_matrix(self.doc, &dict, rect) else {
                    continue;
                };
                // The form's own Matrix is applied inside `form`, so pass
                // only the part that maps its box onto the rectangle.
                self.form(id, &dict, &Dictionary::new(), fit, 0);
            }
        }
    }
}

fn rect(doc: &Document, d: &Dictionary, key: &[u8]) -> Option<[f64; 4]> {
    let (_, o) = doc.dereference(d.get(key).ok()?).ok()?;
    let a = o.as_array().ok()?;
    if a.len() != 4 {
        return None;
    }
    let v: Vec<f64> = a.iter().filter_map(number).collect();
    (v.len() == 4).then(|| {
        [
            v[0].min(v[2]),
            v[1].min(v[3]),
            v[0].max(v[2]),
            v[1].max(v[3]),
        ]
    })
}

/// PDF 32000 12.5.5: transform the form's BBox by its Matrix, then map that
/// box onto the annotation rectangle.
fn appearance_matrix(doc: &Document, form: &Dictionary, rect: [f64; 4]) -> Option<Matrix> {
    let bbox = self::rect(doc, form, b"BBox")?;
    let m = form
        .get(b"Matrix")
        .ok()
        .and_then(|m| m.as_array().ok())
        .and_then(|a| Matrix::from_operands(a))
        .unwrap_or(Matrix::IDENTITY);
    let corners = [
        (bbox[0], bbox[1]),
        (bbox[2], bbox[1]),
        (bbox[0], bbox[3]),
        (bbox[2], bbox[3]),
    ];
    let [a, b, c, d, e, f] = m.0;
    let pts: Vec<(f64, f64)> = corners
        .iter()
        .map(|&(x, y)| (a * x + c * y + e, b * x + d * y + f))
        .collect();
    let x0 = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let x1 = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let y0 = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let y1 = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    let (bw, bh) = (x1 - x0, y1 - y0);
    if bw <= 0.0 || bh <= 0.0 {
        return None;
    }
    let sx = (rect[2] - rect[0]) / bw;
    let sy = (rect[3] - rect[1]) / bh;
    Some(Matrix([
        sx,
        0.0,
        0.0,
        sy,
        rect[0] - x0 * sx,
        rect[1] - y0 * sy,
    ]))
}

fn xobject_ref(doc: &Document, resources: &Dictionary, name: &[u8]) -> Option<ObjectId> {
    let (_, xobjects) = doc.dereference(resources.get(b"XObject").ok()?).ok()?;
    xobjects.as_dict().ok()?.get(name).ok()?.as_reference().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testpdf::Builder;
    use lopdf::dictionary;

    fn sizes(found: &Found, id: ObjectId) -> Vec<(u32, f64, f64)> {
        found.placements[&id]
            .iter()
            .map(|p| (p.page, p.width_pt.round(), p.height_pt.round()))
            .collect()
    }

    #[test]
    fn image_size_comes_from_the_ctm_at_do() {
        let mut b = Builder::new();
        let img = b.jpeg_image(64, 32, 80);
        b.page(
            "q 0.5 0 0 0.5 0 0 cm q 288 0 0 144 36 36 cm /Im0 Do Q Q /Im0 Do",
            &[("Im0", img)],
            None,
        );
        let doc = b.finish();
        let found = find_images(&doc);
        // Inside the halving q: 144 x 72. After both Q: the bare unit square.
        assert_eq!(sizes(&found, img), vec![(1, 144.0, 72.0), (1, 1.0, 1.0)]);
    }

    #[test]
    fn forms_apply_their_matrix_and_nest() {
        let mut b = Builder::new();
        let img = b.jpeg_image(100, 100, 80);
        let inner = b.form("200 0 0 200 0 0 cm /Im0 Do", &[("Im0", img)], None);
        let outer = b.form(
            "/Fm1 Do",
            &[("Fm1", inner)],
            Some([0.5, 0.0, 0.0, 0.5, 0.0, 0.0]),
        );
        b.page("q 2 0 0 2 0 0 cm /Fm0 Do Q", &[("Fm0", outer)], None);
        b.page("/Fm0 Do", &[("Fm0", outer)], None);
        let doc = b.finish();
        let found = find_images(&doc);
        // Page 1: 200 * 0.5 * 2 = 200 pt. Page 2: 200 * 0.5 = 100 pt.
        assert_eq!(
            sizes(&found, img),
            vec![(1, 200.0, 200.0), (2, 100.0, 100.0)]
        );
    }

    #[test]
    fn a_form_that_draws_itself_stops() {
        let mut b = Builder::new();
        let looped = b.doc.new_object_id();
        let img = b.jpeg_image(8, 8, 80);
        let form = lopdf::Stream::new(
            dictionary! {
                "Subtype" => "Form",
                "BBox" => vec![0.into(), 0.into(), 1.into(), 1.into()],
                "Resources" => dictionary! {
                    "XObject" => dictionary! { "Me" => looped, "Im0" => img },
                },
            },
            b"/Im0 Do /Me Do".to_vec(),
        );
        b.doc.objects.insert(looped, Object::Stream(form));
        b.page("/Me Do", &[("Me", looped)], None);
        let found = find_images(&b.finish());
        assert_eq!(found.placements[&img].len(), 1);
    }

    #[test]
    fn unbalanced_restore_and_inline_images_dont_derail_the_walk() {
        let mut b = Builder::new();
        let img = b.jpeg_image(8, 8, 80);
        b.page(
            "Q Q BI /W 1 /H 1 /CS /G /BPC 8 ID z EI 72 0 0 72 0 0 cm /Im0 Do",
            &[("Im0", img)],
            None,
        );
        let found = find_images(&b.finish());
        assert_eq!(found.inline_images, 1);
        assert_eq!(sizes(&found, img), vec![(1, 72.0, 72.0)]);
    }
}
