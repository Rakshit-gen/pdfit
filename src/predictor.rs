//! Undo the `/Predictor` that Flate-compressed images often use (PDF 32000
//! 7.4.4.4). The predictor runs after inflate, row by row.

use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum PredictorError {
    #[error("unknown PNG row filter {0}")]
    BadRowFilter(u8),
    #[error("predictor {0} is not supported")]
    Unsupported(i64),
    #[error("TIFF predictor with {0} bits per component is not supported")]
    TiffDepth(i64),
}

#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub predictor: i64,
    pub colors: i64,
    pub bits_per_component: i64,
    pub columns: i64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            predictor: 1,
            colors: 1,
            bits_per_component: 8,
            columns: 1,
        }
    }
}

impl Params {
    fn row_bytes(&self) -> usize {
        ((self.colors * self.bits_per_component * self.columns + 7) / 8).max(1) as usize
    }

    /// Bytes per whole pixel, at least one, as PNG defines it for filtering.
    fn pixel_bytes(&self) -> usize {
        ((self.colors * self.bits_per_component + 7) / 8).max(1) as usize
    }
}

pub fn decode(data: &[u8], p: Params) -> Result<Vec<u8>, PredictorError> {
    match p.predictor {
        1 => Ok(data.to_vec()),
        2 => tiff(data, p),
        10..=15 => png(data, p),
        other => Err(PredictorError::Unsupported(other)),
    }
}

/// PNG predictors: every row starts with its own filter type byte, whatever
/// the `/Predictor` value says, so 10 to 15 all decode the same way.
fn png(data: &[u8], p: Params) -> Result<Vec<u8>, PredictorError> {
    let row = p.row_bytes();
    let bpp = p.pixel_bytes();
    let mut out = Vec::with_capacity(data.len());
    let mut prev = vec![0u8; row];
    let mut cur = vec![0u8; row];
    // A truncated last row is decoded as far as it goes; some writers do
    // this and readers tolerate it.
    for chunk in data.chunks(row + 1) {
        let (&kind, bytes) = chunk.split_first().expect("chunks are never empty");
        cur[..bytes.len()].copy_from_slice(bytes);
        cur[bytes.len()..].fill(0);
        for i in 0..row {
            let left = if i >= bpp { cur[i - bpp] } else { 0 };
            let up = prev[i];
            let up_left = if i >= bpp { prev[i - bpp] } else { 0 };
            let predicted = match kind {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((left as u16 + up as u16) / 2) as u8,
                4 => paeth(left, up, up_left),
                k => return Err(PredictorError::BadRowFilter(k)),
            };
            cur[i] = cur[i].wrapping_add(predicted);
        }
        out.extend_from_slice(&cur[..bytes.len()]);
        std::mem::swap(&mut prev, &mut cur);
    }
    Ok(out)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).abs();
    let pb = (p - b as i16).abs();
    let pc = (p - c as i16).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// TIFF predictor 2: each sample is stored as the difference from the same
/// component of the pixel to its left.
fn tiff(data: &[u8], p: Params) -> Result<Vec<u8>, PredictorError> {
    if p.bits_per_component != 8 {
        return Err(PredictorError::TiffDepth(p.bits_per_component));
    }
    let row = p.row_bytes();
    let colors = p.colors.max(1) as usize;
    let mut out = data.to_vec();
    for r in out.chunks_mut(row) {
        for i in colors..r.len() {
            r[i] = r[i].wrapping_add(r[i - colors]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(predictor: i64, colors: i64, columns: i64) -> Params {
        Params {
            predictor,
            colors,
            bits_per_component: 8,
            columns,
        }
    }

    /// Encode with one PNG filter type for every row, the way a writer would.
    fn png_encode(raw: &[u8], row: usize, bpp: usize, kind: u8) -> Vec<u8> {
        let mut out = Vec::new();
        let mut prev = vec![0u8; row];
        for r in raw.chunks(row) {
            out.push(kind);
            for i in 0..row {
                let left = if i >= bpp { r[i - bpp] } else { 0 };
                let up = prev[i];
                let up_left = if i >= bpp { prev[i - bpp] } else { 0 };
                let pred = match kind {
                    0 => 0,
                    1 => left,
                    2 => up,
                    3 => ((left as u16 + up as u16) / 2) as u8,
                    _ => paeth(left, up, up_left),
                };
                out.push(r[i].wrapping_sub(pred));
            }
            prev.copy_from_slice(r);
        }
        out
    }

    #[test]
    fn every_png_filter_round_trips() {
        // 5 RGB pixels per row, 4 rows, values chosen to wrap around.
        let raw: Vec<u8> = (0..60u32).map(|i| (i * 97 % 256) as u8).collect();
        for kind in 0..=4 {
            let enc = png_encode(&raw, 15, 3, kind);
            assert_eq!(
                decode(&enc, params(15, 3, 5)).unwrap(),
                raw,
                "filter {kind}"
            );
        }
    }

    #[test]
    fn rows_can_mix_filters() {
        let raw: Vec<u8> = (0..12u8).collect();
        let mut enc = png_encode(&raw[..6], 6, 1, 1);
        // Second row encoded relative to the first with Up.
        enc.push(2);
        enc.extend(
            raw[6..]
                .iter()
                .zip(&raw[..6])
                .map(|(a, b)| a.wrapping_sub(*b)),
        );
        assert_eq!(decode(&enc, params(12, 1, 6)).unwrap(), raw);
    }

    #[test]
    fn tiff_predictor_adds_the_pixel_to_the_left() {
        // Two RGB pixels: (10,20,30) then (+1,+2,+3).
        let enc = [10, 20, 30, 1, 2, 3];
        assert_eq!(
            decode(&enc, params(2, 3, 2)).unwrap(),
            [10, 20, 30, 11, 22, 33]
        );
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        assert_eq!(
            decode(&[9, 1, 2], params(10, 1, 2)),
            Err(PredictorError::BadRowFilter(9))
        );
        assert_eq!(
            decode(&[], params(3, 1, 1)),
            Err(PredictorError::Unsupported(3))
        );
        // A short last row decodes as far as it goes.
        assert_eq!(decode(&[1, 5, 1, 1], params(10, 1, 4)).unwrap(), [5, 6, 7]);
    }
}
