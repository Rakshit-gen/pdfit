//! Builds small PDFs in memory for the unit tests, so the tests run anywhere
//! without fixture files.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};

pub struct Builder {
    pub doc: Document,
    pages_id: ObjectId,
    kids: Vec<Object>,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

/// A smooth, photo-like test pattern, so JPEG has something realistic to
/// chew on and downsampling has detail to lose.
pub fn pattern(w: u32, h: u32, channels: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(w as usize * h as usize * channels);
    for y in 0..h {
        for x in 0..w {
            for c in 0..channels {
                let fx = x as f32 / w as f32;
                let fy = y as f32 / h as f32;
                let t = (fx * 6.0 + c as f32).sin() * (fy * 4.0).cos();
                let n = ((x * 31 + y * 17 + c as u32 * 7) % 13) as f32;
                v.push((128.0 + 100.0 * t + n).clamp(0.0, 255.0) as u8);
            }
        }
    }
    v
}

pub fn jpeg(w: u32, h: u32, channels: usize, quality: u8) -> Vec<u8> {
    let color = match channels {
        1 => jpeg_encoder::ColorType::Luma,
        3 => jpeg_encoder::ColorType::Rgb,
        _ => jpeg_encoder::ColorType::Cmyk,
    };
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, quality)
        .encode(&pattern(w, h, channels), w as u16, h as u16, color)
        .unwrap();
    out
}

impl Builder {
    pub fn new() -> Self {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        Builder {
            doc,
            pages_id,
            kids: Vec::new(),
        }
    }

    pub fn jpeg_image(&mut self, w: u32, h: u32, quality: u8) -> ObjectId {
        let data = jpeg(w, h, 3, quality);
        self.image(w, h, "DeviceRGB", "DCTDecode", data)
    }

    /// Raw 8-bit samples, Flate-compressed, no predictor.
    pub fn flate_image(&mut self, w: u32, h: u32, color_space: &str, channels: usize) -> ObjectId {
        let data = deflate(&pattern(w, h, channels));
        self.image(w, h, color_space, "FlateDecode", data)
    }

    pub fn image(&mut self, w: u32, h: u32, cs: &str, filter: &str, data: Vec<u8>) -> ObjectId {
        let dict = dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => w as i64,
            "Height" => h as i64,
            "ColorSpace" => Object::Name(cs.as_bytes().to_vec()),
            "BitsPerComponent" => 8,
            "Filter" => Object::Name(filter.as_bytes().to_vec()),
        };
        self.doc
            .add_object(Stream::new(dict, data).with_compression(false))
    }

    pub fn form(
        &mut self,
        content: &str,
        xobjects: &[(&str, ObjectId)],
        matrix: Option<[f32; 6]>,
    ) -> ObjectId {
        let mut dict = dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 1000.into(), 1000.into()],
            "Resources" => resources(xobjects, None),
        };
        if let Some(m) = matrix {
            dict.set(
                "Matrix",
                m.iter().map(|&v| Object::Real(v)).collect::<Vec<_>>(),
            );
        }
        self.doc
            .add_object(Stream::new(dict, content.as_bytes().to_vec()))
    }

    /// A page with a content stream, some XObjects and optionally a font.
    pub fn page(
        &mut self,
        content: &str,
        xobjects: &[(&str, ObjectId)],
        font: Option<ObjectId>,
    ) -> ObjectId {
        let content_id = self
            .doc
            .add_object(Stream::new(Dictionary::new(), content.as_bytes().to_vec()));
        let page = dictionary! {
            "Type" => "Page",
            "Parent" => self.pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => content_id,
            "Resources" => resources(xobjects, font),
        };
        let id = self.doc.add_object(page);
        self.kids.push(id.into());
        id
    }

    /// A TrueType font whose program is a stream of made-up bytes; enough to
    /// check that pdfit leaves font programs alone.
    pub fn font(&mut self, program: &[u8]) -> ObjectId {
        let file = self
            .doc
            .add_object(Stream::new(dictionary! {}, program.to_vec()).with_compression(false));
        let descriptor = self.doc.add_object(dictionary! {
            "Type" => "FontDescriptor",
            "FontName" => "TestSans",
            "FontFile2" => file,
        });
        self.doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "TrueType",
            "BaseFont" => "TestSans",
            "FontDescriptor" => descriptor,
        })
    }

    pub fn finish(mut self) -> Document {
        let count = self.kids.len() as i64;
        self.doc.objects.insert(
            self.pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => self.kids,
                "Count" => count,
            }),
        );
        let catalog = self.doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => self.pages_id,
        });
        self.doc.trailer.set("Root", catalog);
        self.doc
    }

    pub fn bytes(self) -> Vec<u8> {
        let mut doc = self.finish();
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }
}

fn resources(xobjects: &[(&str, ObjectId)], font: Option<ObjectId>) -> Dictionary {
    let mut x = Dictionary::new();
    for (name, id) in xobjects {
        x.set(name.as_bytes().to_vec(), *id);
    }
    let mut r = dictionary! { "XObject" => x };
    if let Some(f) = font {
        r.set("Font", dictionary! { "F1" => f });
    }
    r
}

pub fn deflate(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}
