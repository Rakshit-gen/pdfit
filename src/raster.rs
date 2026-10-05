//! Turn an image XObject into plain 8-bit pixels, or say why pdfit leaves it
//! alone.
//!
//! The rule throughout: produce what a viewer would show, not what the
//! bytes "should" mean. If a CMYK JPEG renders inverted in Acrobat because
//! its /Decode array is missing, it stays inverted here too; pdfit only
//! changes how many pixels there are, never what they look like.

use std::fmt;
use std::io::Read;

use lopdf::{Dictionary, Document, Object, Stream};

use crate::predictor::{self, Params};

/// Pixels after decoding: gray or RGB, 8 bits, row-major, no padding.
#[derive(Clone, Debug, PartialEq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub data: Vec<u8>,
}

/// How the samples in the file map to colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Gray,
    Rgb,
    Cmyk,
}

impl Model {
    pub fn components(self) -> usize {
        match self {
            Model::Gray => 1,
            Model::Rgb => 3,
            Model::Cmyk => 4,
        }
    }
}

/// Why an image is left as it is. These end up in the report, so they say
/// it in words a person can act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    /// JBIG2, CCITT and JPEG 2000 are already compact, and pdfit has no
    /// pure-Rust decoder it trusts for them.
    Codec(&'static str),
    OneBit,
    StencilMask,
    Palette,
    ColorSpace(String),
    ColorKeyMask,
    Depth(i64),
    Broken(String),
}

impl fmt::Display for Skip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Skip::Codec(c) => write!(f, "{c} image, left as it is"),
            Skip::OneBit => write!(
                f,
                "1-bit black and white, already small and would blur as JPEG"
            ),
            Skip::StencilMask => write!(f, "stencil mask, left as it is"),
            Skip::Palette => write!(
                f,
                "palette image (usually a chart or screenshot), would smear as JPEG"
            ),
            Skip::ColorSpace(c) => write!(f, "{c} colour space isn't supported, left as it is"),
            Skip::ColorKeyMask => write!(
                f,
                "colour-key transparency needs exact colours, JPEG can't keep them"
            ),
            Skip::Depth(b) => write!(f, "{b}-bit samples aren't supported, left as it is"),
            Skip::Broken(why) => write!(f, "couldn't decode it ({why}), left as it is"),
        }
    }
}

pub fn filters(dict: &Dictionary) -> Vec<Vec<u8>> {
    match dict.get(b"Filter") {
        Ok(Object::Name(n)) => vec![n.clone()],
        Ok(Object::Array(a)) => a
            .iter()
            .filter_map(|o| o.as_name().ok().map(<[u8]>::to_vec))
            .collect(),
        _ => Vec::new(),
    }
}

fn int(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<i64> {
    let (_, o) = doc.dereference(dict.get(key).ok()?).ok()?;
    o.as_i64().ok()
}

/// Work out the colour model, or why the image is out of scope, without
/// decoding any pixels. Cheap enough to run over every image up front.
pub fn classify(doc: &Document, dict: &Dictionary) -> Result<Model, Skip> {
    for f in filters(dict) {
        match f.as_slice() {
            b"JBIG2Decode" => return Err(Skip::Codec("JBIG2")),
            b"CCITTFaxDecode" => return Err(Skip::Codec("CCITT fax")),
            b"JPXDecode" => return Err(Skip::Codec("JPEG 2000")),
            _ => {}
        }
    }
    if matches!(dict.get(b"ImageMask"), Ok(Object::Boolean(true))) {
        return Err(Skip::StencilMask);
    }
    // An array /Mask means "pixels of exactly this colour are transparent";
    // after JPEG they would no longer be exactly that colour.
    if matches!(dict.get(b"Mask"), Ok(Object::Array(_))) {
        return Err(Skip::ColorKeyMask);
    }
    let is_dct = filters(dict).iter().any(|f| f == b"DCTDecode");
    let bpc = int(doc, dict, b"BitsPerComponent").unwrap_or(8);
    if bpc == 1 {
        return Err(Skip::OneBit);
    }
    if bpc != 8 && !is_dct {
        return Err(Skip::Depth(bpc));
    }
    let cs = match dict.get(b"ColorSpace") {
        Ok(o) => doc.dereference(o).map(|(_, o)| o.clone()).ok(),
        Err(_) => None,
    };
    match cs {
        // A JPEG knows its own component count; without a colour space
        // (only legal for JPX, but seen in the wild) trust it.
        None if is_dct => Ok(Model::Rgb),
        None => Err(Skip::ColorSpace("missing".into())),
        Some(cs) => model_of(doc, &cs),
    }
}

fn model_of(doc: &Document, cs: &Object) -> Result<Model, Skip> {
    let named = |n: &[u8]| match n {
        b"DeviceGray" | b"G" | b"CalGray" => Ok(Model::Gray),
        b"DeviceRGB" | b"RGB" | b"CalRGB" => Ok(Model::Rgb),
        b"DeviceCMYK" | b"CMYK" => Ok(Model::Cmyk),
        b"Indexed" | b"I" => Err(Skip::Palette),
        other => Err(Skip::ColorSpace(
            String::from_utf8_lossy(other).into_owned(),
        )),
    };
    match cs {
        Object::Name(n) => named(n),
        Object::Array(a) => {
            let family = a.first().and_then(|f| f.as_name().ok()).unwrap_or(b"");
            match family {
                b"ICCBased" => {
                    let n = a
                        .get(1)
                        .and_then(|r| doc.dereference(r).ok())
                        .and_then(|(_, o)| o.as_stream().ok())
                        .and_then(|s| s.dict.get(b"N").ok()?.as_i64().ok());
                    match n {
                        Some(1) => Ok(Model::Gray),
                        Some(3) => Ok(Model::Rgb),
                        Some(4) => Ok(Model::Cmyk),
                        _ => Err(Skip::ColorSpace("ICCBased without a valid N".into())),
                    }
                }
                // Calibrated spaces keep their numbers per component, so
                // the samples can be re-encoded as they are.
                b"CalGray" => Ok(Model::Gray),
                b"CalRGB" => Ok(Model::Rgb),
                other => named(other),
            }
        }
        _ => Err(Skip::ColorSpace("unreadable".into())),
    }
}

/// Decode an image to pixels. Gray and RGB come out with the same samples
/// the file holds (the image's /Decode array and colour space still apply
/// to them, and are kept). CMYK is converted to RGB the way viewers do,
/// with /Decode applied, because there's no point keeping four channels
/// for a file that's going to a screen or an upload form.
pub fn decode(doc: &Document, stream: &Stream) -> Result<(Pixels, Model), Skip> {
    let dict = &stream.dict;
    let model = classify(doc, dict)?;
    let width = int(doc, dict, b"Width")
        .filter(|&w| w > 0)
        .ok_or(Skip::Broken("no width".into()))?;
    let height = int(doc, dict, b"Height")
        .filter(|&h| h > 0)
        .ok_or(Skip::Broken("no height".into()))?;
    if width * height > 200_000_000 {
        return Err(Skip::Broken("more than 200 megapixels".into()));
    }
    let (width, height) = (width as u32, height as u32);
    let n = model.components();
    let expected = width as usize * height as usize * n;

    let filters = filters(dict);
    let parms = decode_parms(doc, dict, filters.len());
    let mut data = stream.content.clone();
    let mut from_jpeg = false;
    for (i, f) in filters.iter().enumerate() {
        data = match f.as_slice() {
            b"FlateDecode" | b"Fl" => {
                let raw = inflate(&data, expected * 2 + height as usize + 1024)?;
                let p = params(parms[i].as_ref());
                predictor::decode(&raw, p).map_err(|e| Skip::Broken(e.to_string()))?
            }
            b"DCTDecode" | b"DCT" => {
                if i + 1 != filters.len() {
                    return Err(Skip::Broken("DCTDecode isn't the last filter".into()));
                }
                from_jpeg = true;
                jpeg(&data, n)?
            }
            other => other_filter(&data, other, parms[i].clone())?,
        };
    }
    if data.len() < expected {
        // Short image data is common in damaged files; viewers pad with
        // black, but re-encoding a guess isn't worth it.
        return Err(Skip::Broken(format!(
            "image data is short: {} of {} bytes",
            data.len(),
            expected
        )));
    }
    data.truncate(expected);

    if model == Model::Cmyk {
        if from_jpeg {
            // jpeg-decoder returns 255 - sample for every 4-channel JPEG
            // (it assumes Adobe's inverted storage). Viewers apply /Decode
            // to libjpeg's raw samples instead, so undo the inversion first.
            for v in &mut data {
                *v = 255 - *v;
            }
        }
        let decode = decode_array(doc, dict, 4);
        let rgb = cmyk_to_rgb(&data, &decode);
        return Ok((
            Pixels {
                width,
                height,
                channels: 3,
                data: rgb,
            },
            model,
        ));
    }
    Ok((
        Pixels {
            width,
            height,
            channels: n as u8,
            data,
        },
        model,
    ))
}

fn decode_parms(doc: &Document, dict: &Dictionary, count: usize) -> Vec<Option<Dictionary>> {
    let as_dict = |o: &Object| {
        doc.dereference(o)
            .ok()
            .and_then(|(_, o)| o.as_dict().ok().cloned())
    };
    let mut out = vec![None; count];
    match dict.get(b"DecodeParms").or_else(|_| dict.get(b"DP")) {
        Ok(Object::Array(a)) => {
            for (slot, o) in out.iter_mut().zip(a) {
                *slot = as_dict(o);
            }
        }
        Ok(o) => {
            if let Some(first) = out.first_mut() {
                *first = as_dict(o);
            }
        }
        Err(_) => {}
    }
    out
}

fn params(d: Option<&Dictionary>) -> Params {
    let mut p = Params::default();
    if let Some(d) = d {
        let get = |k: &[u8], v: i64| d.get(k).and_then(Object::as_i64).unwrap_or(v);
        p.predictor = get(b"Predictor", 1);
        p.colors = get(b"Colors", 1);
        p.bits_per_component = get(b"BitsPerComponent", 8);
        p.columns = get(b"Columns", 1);
    }
    p
}

/// Inflate, keeping whatever came out before a truncated or corrupt tail.
/// The limit stops a small stream from expanding into gigabytes.
fn inflate(data: &[u8], limit: usize) -> Result<Vec<u8>, Skip> {
    let mut out = Vec::new();
    let mut z = flate2::read::ZlibDecoder::new(data).take(limit as u64);
    if let Err(e) = z.read_to_end(&mut out)
        && out.is_empty()
    {
        return Err(Skip::Broken(format!("Flate data is corrupt: {e}")));
    }
    Ok(out)
}

fn other_filter(data: &[u8], name: &[u8], parms: Option<Dictionary>) -> Result<Vec<u8>, Skip> {
    let mut d = Dictionary::new();
    d.set("Filter", Object::Name(name.to_vec()));
    if let Some(p) = parms {
        d.set("DecodeParms", p);
    }
    let s = Stream::new(d, data.to_vec());
    s.decompressed_content()
        .map_err(|e| Skip::Broken(format!("{} filter: {e}", String::from_utf8_lossy(name))))
}

fn jpeg(data: &[u8], components: usize) -> Result<Vec<u8>, Skip> {
    let mut d = jpeg_decoder::Decoder::new(data);
    let pixels = d.decode().map_err(|e| Skip::Broken(format!("JPEG: {e}")))?;
    let info = d.info().ok_or(Skip::Broken("JPEG has no header".into()))?;
    let got = info.pixel_format.pixel_bytes();
    if got != components {
        return Err(Skip::Broken(format!(
            "JPEG has {got} channels but the colour space has {components}"
        )));
    }
    Ok(pixels)
}

/// The image's /Decode array as (min, max) pairs, defaulting to (0, 1).
pub fn decode_array(doc: &Document, dict: &Dictionary, n: usize) -> Vec<(f32, f32)> {
    let arr: Vec<f32> = dict
        .get(b"Decode")
        .ok()
        .and_then(|o| doc.dereference(o).ok())
        .and_then(|(_, o)| o.as_array().ok().cloned())
        .map(|a| a.iter().filter_map(|o| o.as_float().ok()).collect())
        .unwrap_or_default();
    (0..n)
        .map(|i| match (arr.get(2 * i), arr.get(2 * i + 1)) {
            (Some(&a), Some(&b)) => (a, b),
            _ => (0.0, 1.0),
        })
        .collect()
}

/// CMYK to RGB with the polynomial fit pdf.js uses for DeviceCMYK (an
/// approximation of a US Web Coated press profile). The naive
/// `(1 - c) * (1 - k)` formula makes everything too bright and saturated
/// next to what Acrobat or pdf.js show.
fn cmyk_to_rgb(cmyk: &[u8], decode: &[(f32, f32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(cmyk.len() / 4 * 3);
    let apply = |v: u8, (lo, hi): (f32, f32)| (lo + v as f32 / 255.0 * (hi - lo)).clamp(0.0, 1.0);
    for px in cmyk.chunks_exact(4) {
        let c = apply(px[0], decode[0]);
        let m = apply(px[1], decode[1]);
        let y = apply(px[2], decode[2]);
        let k = apply(px[3], decode[3]);
        let r = 255.0
            + c * (-4.387332384609988 * c
                + 54.48615194189176 * m
                + 18.82290502165302 * y
                + 212.25662451639585 * k
                - 285.2331026137004)
            + m * (1.7149763477362134 * m
                - 5.6096736904047315 * y
                - 17.873870861415444 * k
                - 5.497006427196366)
            + y * (-2.5217340131683033 * y - 21.248923337353073 * k - 17.5119270841813)
            + k * (-21.86122147463605 * k - 189.48180835922747);
        let g = 255.0
            + c * (8.841041422036149 * c
                + 60.118027045597366 * m
                + 6.871425592049007 * y
                + 31.159100130055922 * k
                - 79.2970844816548)
            + m * (-15.310361306967817 * m + 17.575251261109482 * y + 131.35250912493976 * k
                - 190.9453302588951)
            + y * (4.444339102852739 * y + 9.8632861493405 * k - 24.86741582555878)
            + k * (-20.737325471181034 * k - 187.80453709719578);
        let b = 255.0
            + c * (0.8842522430003296 * c + 8.078677503112928 * m + 30.89978309703729 * y
                - 0.23883238689178934 * k
                - 14.183576799673286)
            + m * (10.49593273432072 * m + 63.02378494754052 * y + 50.606957656360734 * k
                - 112.23884253719248)
            + y * (0.03296041114873217 * y + 115.60384449646641 * k - 193.58209356861505)
            + k * (-22.33816807309886 * k - 180.12613974708367);
        for v in [r, g, b] {
            out.push(v.round().clamp(0.0, 255.0) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testpdf::{Builder, deflate, pattern};
    use lopdf::dictionary;

    fn image(dict: Dictionary, data: Vec<u8>) -> (Document, Stream) {
        (Builder::new().finish(), Stream::new(dict, data))
    }

    fn base(w: i64, h: i64, cs: Object) -> Dictionary {
        dictionary! {
            "Subtype" => "Image",
            "Width" => w,
            "Height" => h,
            "BitsPerComponent" => 8,
            "ColorSpace" => cs,
        }
    }

    #[test]
    fn flate_with_png_predictor_in_a_parms_array() {
        // lopdf's own decoder ignores /DecodeParms when it's an array; this
        // is the case that made pdfit run its own Flate path.
        let raw = pattern(4, 3, 3);
        let mut enc = Vec::new();
        for row in raw.chunks(12) {
            enc.push(1); // Sub
            for i in 0..12 {
                let left = if i >= 3 { row[i - 3] } else { 0 };
                enc.push(row[i].wrapping_sub(left));
            }
        }
        let mut d = base(4, 3, "DeviceRGB".into());
        d.set("Filter", vec![Object::Name(b"FlateDecode".to_vec())]);
        d.set(
            "DecodeParms",
            vec![Object::Dictionary(dictionary! {
                "Predictor" => 15, "Colors" => 3, "Columns" => 4,
            })],
        );
        let (doc, s) = image(d, deflate(&enc));
        let (px, model) = decode(&doc, &s).unwrap();
        assert_eq!(model, Model::Rgb);
        assert_eq!(px.data, raw);
    }

    fn cmyk_white_jpeg() -> Vec<u8> {
        // True CMYK (0,0,0,0) is paper white. jpeg-encoder stores it
        // Adobe-style, inverted, as 255.
        let mut out = Vec::new();
        jpeg_encoder::Encoder::new(&mut out, 90)
            .encode(&[0u8; 16 * 16 * 4], 16, 16, jpeg_encoder::ColorType::Cmyk)
            .unwrap();
        out
    }

    #[test]
    fn inverted_cmyk_jpeg_with_a_decode_array_renders_white() {
        let mut d = base(16, 16, "DeviceCMYK".into());
        d.set("Filter", "DCTDecode");
        d.set(
            "Decode",
            vec![
                1.into(),
                0.into(),
                1.into(),
                0.into(),
                1.into(),
                0.into(),
                1.into(),
                0.into(),
            ],
        );
        let (doc, s) = image(d, cmyk_white_jpeg());
        let (px, model) = decode(&doc, &s).unwrap();
        assert_eq!(model, Model::Cmyk);
        assert_eq!(px.channels, 3);
        assert!(px.data.iter().all(|&v| v >= 250), "{:?}", &px.data[..6]);
    }

    #[test]
    fn inverted_cmyk_jpeg_without_decode_stays_dark_like_in_a_viewer() {
        let mut d = base(16, 16, "DeviceCMYK".into());
        d.set("Filter", "DCTDecode");
        let (doc, s) = image(d, cmyk_white_jpeg());
        let (px, _) = decode(&doc, &s).unwrap();
        assert!(px.data.iter().all(|&v| v <= 40), "{:?}", &px.data[..6]);
    }

    #[test]
    fn gray_jpeg_keeps_one_channel() {
        let mut d = base(20, 10, "DeviceGray".into());
        d.set("Filter", "DCTDecode");
        let (doc, s) = image(d, crate::testpdf::jpeg(20, 10, 1, 90));
        let (px, _) = decode(&doc, &s).unwrap();
        assert_eq!(
            (px.width, px.height, px.channels, px.data.len()),
            (20, 10, 1, 200)
        );
    }

    #[test]
    fn icc_based_takes_its_model_from_n() {
        let mut b = Builder::new();
        let icc = b
            .doc
            .add_object(Stream::new(dictionary! { "N" => 4 }, vec![]));
        let doc = b.finish();
        let cs = Object::Array(vec!["ICCBased".into(), icc.into()]);
        assert_eq!(classify(&doc, &base(1, 1, cs)), Ok(Model::Cmyk));
    }

    #[test]
    fn out_of_scope_images_say_why() {
        let doc = Builder::new().finish();
        let mut d = base(8, 8, "DeviceGray".into());
        d.set("Filter", "JBIG2Decode");
        assert_eq!(classify(&doc, &d), Err(Skip::Codec("JBIG2")));

        let mut d = base(8, 8, "DeviceGray".into());
        d.set("BitsPerComponent", 1);
        assert_eq!(classify(&doc, &d), Err(Skip::OneBit));

        let pal = Object::Array(vec![
            "Indexed".into(),
            "DeviceRGB".into(),
            1.into(),
            Object::string_literal(vec![0; 6]),
        ]);
        assert_eq!(classify(&doc, &base(8, 8, pal)), Err(Skip::Palette));

        let mut d = base(8, 8, "DeviceRGB".into());
        d.set(
            "Mask",
            vec![0.into(), 0.into(), 0.into(), 0.into(), 0.into(), 0.into()],
        );
        assert_eq!(classify(&doc, &d), Err(Skip::ColorKeyMask));

        let sep = Object::Array(vec!["Separation".into(), "Gold".into()]);
        assert!(matches!(
            classify(&doc, &base(8, 8, sep)),
            Err(Skip::ColorSpace(_))
        ));
    }

    #[test]
    fn short_data_is_reported_not_guessed() {
        let mut d = base(10, 10, "DeviceRGB".into());
        d.set("Filter", "FlateDecode");
        let (doc, s) = image(d, deflate(&[7; 30]));
        assert!(matches!(decode(&doc, &s), Err(Skip::Broken(_))));
    }
}
