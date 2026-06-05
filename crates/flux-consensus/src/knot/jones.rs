//! Jones polynomial — Laurent polynomial knot invariant.
//!
//! Lifted from `QTFT/blockchain/knot.rs::JonesPolynomial` (2026-05-29).
//! The polynomial is stored as parallel `coefficients`/`powers` vectors —
//! sparse representation, fine for the small polynomials that arise from
//! transaction knots.

use num_complex::Complex64;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A Jones polynomial `V(t) = Σ coefficients[i] · t^powers[i]`.
///
/// Stored sparse; powers may be negative (Laurent). `coefficients` and
/// `powers` are parallel and must have equal length — constructors enforce.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JonesPolynomial {
    /// Coefficient list.
    pub coefficients: Vec<f64>,
    /// Power list, parallel to `coefficients`.
    pub powers: Vec<i32>,
}

impl JonesPolynomial {
    /// Build a polynomial from parallel coefficient/power vectors.
    ///
    /// Panics if the vectors have differing lengths.
    pub fn new(coefficients: Vec<f64>, powers: Vec<i32>) -> Self {
        assert_eq!(
            coefficients.len(),
            powers.len(),
            "coefficients and powers must be parallel"
        );
        Self { coefficients, powers }
    }

    /// Jones polynomial of the unknot — `V(t) = 1`.
    pub fn unknot() -> Self {
        Self::new(vec![1.0], vec![0])
    }

    /// Jones polynomial of the trefoil knot — `V(t) = t + t³ − t⁴`.
    pub fn trefoil() -> Self {
        Self::new(vec![1.0, 1.0, -1.0], vec![1, 3, 4])
    }

    /// Jones polynomial of the Hopf link (integer approximation):
    /// `V(t) ≈ −t − t³` (true value `−t^{5/2} − t^{1/2}`).
    pub fn hopf_link() -> Self {
        Self::new(vec![-1.0, -1.0], vec![1, 3])
    }

    /// Evaluate at a real `t`.
    pub fn evaluate(&self, t: f64) -> f64 {
        self.coefficients
            .iter()
            .zip(self.powers.iter())
            .map(|(c, p)| c * t.powi(*p))
            .sum()
    }

    /// Evaluate at a complex `t`.
    pub fn evaluate_complex(&self, t: Complex64) -> Complex64 {
        self.coefficients
            .iter()
            .zip(self.powers.iter())
            .map(|(c, p)| Complex64::new(*c, 0.0) * t.powi(*p))
            .sum()
    }

    /// Multiply two polynomials. Result is normalised: sorted by power and
    /// duplicate powers merged.
    pub fn multiply(&self, other: &Self) -> Self {
        let mut acc: HashMap<i32, f64> = HashMap::new();
        for (c1, p1) in self.coefficients.iter().zip(&self.powers) {
            for (c2, p2) in other.coefficients.iter().zip(&other.powers) {
                *acc.entry(p1 + p2).or_insert(0.0) += c1 * c2;
            }
        }
        let mut pairs: Vec<(i32, f64)> = acc.into_iter().collect();
        pairs.sort_by_key(|(p, _)| *p);
        Self {
            powers: pairs.iter().map(|(p, _)| *p).collect(),
            coefficients: pairs.iter().map(|(_, c)| *c).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknot_is_one_everywhere() {
        let v = JonesPolynomial::unknot();
        assert!((v.evaluate(1.0) - 1.0).abs() < 1e-12);
        assert!((v.evaluate(-1.0) - 1.0).abs() < 1e-12);
        assert!((v.evaluate(2.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn trefoil_at_t_one() {
        // V_trefoil(1) = 1 + 1 - 1 = 1
        let v = JonesPolynomial::trefoil();
        assert!((v.evaluate(1.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn trefoil_at_t_minus_one() {
        // V_trefoil(-1) = -1 - 1 - 1 = -3
        let v = JonesPolynomial::trefoil();
        assert!((v.evaluate(-1.0) - (-3.0)).abs() < 1e-12);
    }

    #[test]
    fn multiply_unknot_is_identity() {
        let v = JonesPolynomial::trefoil();
        let prod = v.multiply(&JonesPolynomial::unknot());
        assert!((prod.evaluate(1.0) - v.evaluate(1.0)).abs() < 1e-12);
        assert!((prod.evaluate(-1.0) - v.evaluate(-1.0)).abs() < 1e-12);
    }

    #[test]
    fn complex_evaluation_at_i() {
        // V_unknot(i) = 1
        let v = JonesPolynomial::unknot();
        let z = v.evaluate_complex(Complex64::new(0.0, 1.0));
        assert!((z.re - 1.0).abs() < 1e-12);
        assert!(z.im.abs() < 1e-12);
    }
}
