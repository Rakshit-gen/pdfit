//! PDF transformation matrices, `[a b c d e f]`, in the row-vector form the
//! spec uses: a point is `[x y 1] * M`.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix(pub [f64; 6]);

impl Matrix {
    pub const IDENTITY: Matrix = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    /// `self` applied first, then `other`. The `cm` operator sets the CTM to
    /// `operand.then(ctm)`, which is the order that trips people up.
    pub fn then(self, other: Matrix) -> Matrix {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = other.0;
        Matrix([
            a * a2 + b * c2,
            a * b2 + b * d2,
            c * a2 + d * c2,
            c * b2 + d * d2,
            e * a2 + f * c2 + e2,
            e * b2 + f * d2 + f2,
        ])
    }

    /// The size, in the target space, of the unit square drawn through this
    /// matrix. Images are always painted into the unit square, so under the
    /// CTM this is how big the image appears on the page, in points.
    /// Rotation and skew are handled by measuring the transformed edges.
    pub fn unit_square_size(self) -> (f64, f64) {
        let [a, b, c, d, _, _] = self.0;
        ((a * a + b * b).sqrt(), (c * c + d * d).sqrt())
    }

    pub fn from_operands(ops: &[lopdf::Object]) -> Option<Matrix> {
        if ops.len() != 6 {
            return None;
        }
        let mut m = [0.0; 6];
        for (slot, op) in m.iter_mut().zip(ops) {
            *slot = number(op)?;
        }
        Some(Matrix(m))
    }
}

pub fn number(o: &lopdf::Object) -> Option<f64> {
    match o {
        lopdf::Object::Integer(i) => Some(*i as f64),
        lopdf::Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn scale_then_translate_is_not_translate_then_scale() {
        let scale = Matrix([2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
        let shift = Matrix([1.0, 0.0, 0.0, 1.0, 10.0, 0.0]);
        assert_eq!(scale.then(shift).0[4], 10.0);
        assert_eq!(shift.then(scale).0[4], 20.0);
    }

    #[test]
    fn image_size_survives_rotation() {
        // 200 x 100 pt, rotated 90 degrees: a=0 b=200 c=-100 d=0.
        let m = Matrix([0.0, 200.0, -100.0, 0.0, 300.0, 50.0]);
        let (w, h) = m.unit_square_size();
        assert!(close(w, 200.0) && close(h, 100.0));
    }

    #[test]
    fn nested_scales_multiply() {
        let page = Matrix([0.5, 0.0, 0.0, 0.5, 0.0, 0.0]);
        let image = Matrix([400.0, 0.0, 0.0, 300.0, 72.0, 72.0]);
        let (w, h) = image.then(page).unit_square_size();
        assert!(close(w, 200.0) && close(h, 150.0));
    }

    #[test]
    fn operands_must_be_six_numbers() {
        use lopdf::Object::{Integer, Name, Real};
        let ok = [
            Integer(1),
            Real(0.0),
            Real(0.0),
            Integer(1),
            Integer(5),
            Real(6.5),
        ];
        assert_eq!(
            Matrix::from_operands(&ok),
            Some(Matrix([1.0, 0.0, 0.0, 1.0, 5.0, 6.5]))
        );
        assert_eq!(Matrix::from_operands(&ok[..5]), None);
        let bad = [
            Integer(1),
            Name(b"x".to_vec()),
            Real(0.0),
            Integer(1),
            Integer(5),
            Real(6.5),
        ];
        assert_eq!(Matrix::from_operands(&bad), None);
    }
}
