use std::collections::BTreeMap;
use std::fmt;
use std::ops::{Div, DivAssign, Mul, MulAssign, Neg};

/// A unit of measurement represented as a map from dimension names to integer exponents.
/// An empty map represents a dimensionless scalar.
///
/// Examples:
/// - `{}` = dimensionless scalar
/// - `{"usd": 1}` = USD
/// - `{"usd": 1, "year": -1}` = USD per year
/// - `{"sek": 1, "usd": -1}` = SEK/USD exchange rate
/// - `{"usd": 2}` = USD²
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Unit(BTreeMap<String, i32>);

impl Unit {
    /// Dimensionless scalar (empty exponent map).
    pub fn scalar() -> Self {
        Unit(BTreeMap::new())
    }

    /// A unit with a single dimension at exponent 1.
    pub fn single(dim: impl Into<String>) -> Self {
        let mut m = BTreeMap::new();
        m.insert(dim.into(), 1);
        Unit(m)
    }

    /// Scale all exponents by `n`. Returns scalar when `n == 0`.
    pub fn pow(&self, n: i32) -> Self {
        if n == 0 {
            return Self::scalar();
        }
        Unit(self.0.iter().map(|(k, &v)| (k.clone(), v * n)).collect())
    }

    /// True if this is a dimensionless scalar (empty exponent map).
    pub fn is_scalar(&self) -> bool {
        self.0.is_empty()
    }

    /// True if both units have identical exponent maps (required for addition/subtraction).
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        self.0 == other.0
    }

    /// The exponent of a named dimension (0 if absent).
    pub fn exponent_of(&self, dim: &str) -> i32 {
        self.0.get(dim).copied().unwrap_or(0)
    }

    /// Iterate all (dimension, exponent) pairs. All exponents are non-zero by construction.
    pub fn dims(&self) -> impl Iterator<Item = (&str, i32)> {
        self.0.iter().map(|(k, &v)| (k.as_str(), v))
    }

    /// Parse a simple unit annotation: `"dim"`, `"dim/dim"`, or `"1"`/`""` for scalar.
    pub fn parse(s: &str) -> Result<Self, String> {
        if s.is_empty() || s == "1" {
            return Ok(Self::scalar());
        }
        if let Some((num, den)) = s.split_once('/') {
            if num.is_empty() || den.is_empty() {
                return Err(format!("cannot parse unit: {s:?}"));
            }
            Ok(Self::single(num) / Self::single(den))
        } else {
            Ok(Self::single(s))
        }
    }
}

fn combine(mut a: BTreeMap<String, i32>, b: &BTreeMap<String, i32>, sign: i32) -> Unit {
    for (k, &v) in b {
        let e = a.entry(k.clone()).or_insert(0);
        *e += v * sign;
    }
    a.retain(|_, v| *v != 0);
    Unit(a)
}

impl Mul<Unit> for Unit {
    type Output = Unit;
    fn mul(self, rhs: Unit) -> Unit {
        combine(self.0, &rhs.0, 1)
    }
}

impl MulAssign<Unit> for Unit {
    fn mul_assign(&mut self, rhs: Unit) {
        let lhs = std::mem::take(self);
        *self = lhs * rhs;
    }
}

impl Div<Unit> for Unit {
    type Output = Unit;
    fn div(self, rhs: Unit) -> Unit {
        combine(self.0, &rhs.0, -1)
    }
}

impl DivAssign<Unit> for Unit {
    fn div_assign(&mut self, rhs: Unit) {
        let lhs = std::mem::take(self);
        *self = lhs / rhs;
    }
}

impl Neg for Unit {
    type Output = Unit;
    fn neg(self) -> Unit {
        self.pow(-1)
    }
}

impl Mul<i32> for Unit {
    type Output = Unit;
    fn mul(self, rhs: i32) -> Unit {
        self.pow(rhs)
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return write!(f, "1");
        }

        let fmt_term = |name: &str, abs_exp: i32| -> String {
            if abs_exp == 1 {
                name.to_string()
            } else {
                format!("{name}^{abs_exp}")
            }
        };

        let pos: Vec<String> = self
            .0
            .iter()
            .filter(|&(_, e)| *e > 0)
            .map(|(k, e)| fmt_term(k, *e))
            .collect();
        let neg: Vec<String> = self
            .0
            .iter()
            .filter(|&(_, e)| *e < 0)
            .map(|(k, e)| fmt_term(k, -*e))
            .collect();

        if pos.is_empty() {
            write!(f, "1")?;
        } else {
            write!(f, "{}", pos.join("*"))?;
        }

        if !neg.is_empty() {
            if neg.len() == 1 {
                write!(f, "/{}", neg[0])?;
            } else {
                write!(f, "/({})", neg.join("*"))?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Unit {
        Unit::parse(s).unwrap()
    }

    // ---- constructors ----

    #[test]
    fn scalar_is_empty() {
        let s = Unit::scalar();
        assert!(s.is_scalar());
        assert_eq!(s.exponent_of("usd"), 0);
        assert_eq!(s.exponent_of("anything"), 0);
        assert_eq!(s.dims().count(), 0);
    }

    #[test]
    fn single_has_one_dim_at_exp_one() {
        let s = Unit::single("usd");
        assert!(!s.is_scalar());
        assert_eq!(s.exponent_of("usd"), 1);
        assert_eq!(s.exponent_of("sek"), 0);
        assert_eq!(s.dims().count(), 1);
    }

    // ---- Mul ----

    #[test]
    fn mul_by_scalar_identity_right() {
        assert_eq!(Unit::single("usd") * Unit::scalar(), Unit::single("usd"));
    }

    #[test]
    fn mul_by_scalar_identity_left() {
        assert_eq!(Unit::scalar() * Unit::single("usd"), Unit::single("usd"));
    }

    #[test]
    fn mul_cancels_matching_currency_dim() {
        // sek * (usd/sek) = usd
        let result = Unit::single("sek") * u("usd/sek");
        assert_eq!(result, Unit::single("usd"));
    }

    #[test]
    fn mul_accumulates_same_dim() {
        let result = Unit::single("usd") * Unit::single("usd");
        assert_eq!(result.exponent_of("usd"), 2);
        assert!(result.exponent_of("sek") == 0);
    }

    #[test]
    fn mul_cancels_time_dim() {
        // (usd/year) * year = usd
        let result = u("usd/year") * Unit::single("year");
        assert_eq!(result, Unit::single("usd"));
    }

    #[test]
    fn mul_no_cancellation_when_different_dim() {
        // (usd/year) * usd = usd²/year
        let result = u("usd/year") * Unit::single("usd");
        assert_eq!(result.exponent_of("usd"), 2);
        assert_eq!(result.exponent_of("year"), -1);
    }

    // ---- Div ----

    #[test]
    fn div_by_scalar_is_identity() {
        assert_eq!(Unit::single("usd") / Unit::scalar(), Unit::single("usd"));
    }

    #[test]
    fn div_self_gives_scalar() {
        assert!((Unit::single("usd") / Unit::single("usd")).is_scalar());
    }

    #[test]
    fn div_reduces_exponent() {
        // usd² / usd = usd
        let result = Unit::single("usd").pow(2) / Unit::single("usd");
        assert_eq!(result, Unit::single("usd"));
    }

    #[test]
    fn inverted() {
        // 1 / usd = usd^-1
        assert_eq!(
            Unit::scalar() / Unit::single("usd"),
            Unit::single("usd").pow(-1)
        );
    }

    // ---- Neg ----

    #[test]
    fn neg_negates_single_exponent() {
        let result = -Unit::single("usd");
        assert_eq!(result.exponent_of("usd"), -1);
    }

    #[test]
    fn neg_compound_unit() {
        // -(usd/year) = year/usd  →  {usd:-1, year:1}
        let result = -u("usd/year");
        assert_eq!(result.exponent_of("usd"), -1);
        assert_eq!(result.exponent_of("year"), 1);
    }

    #[test]
    fn double_neg_is_identity() {
        let x = Unit::single("usd");
        let neg_x = -x;
        let neg_neg_x = -neg_x;
        assert_eq!(neg_neg_x, Unit::single("usd"));
    }

    // ---- pow ----

    #[test]
    fn pow_two_doubles_exponent() {
        assert_eq!(Unit::single("usd").pow(2).exponent_of("usd"), 2);
    }

    #[test]
    fn pow_zero_gives_scalar() {
        assert!(Unit::single("usd").pow(0).is_scalar());
    }

    #[test]
    fn pow_negative_one_equals_neg() {
        assert_eq!(Unit::single("usd").pow(-1), -Unit::single("usd"));
    }

    #[test]
    fn pow_compound_unit() {
        let result = u("usd/year").pow(2);
        assert_eq!(result.exponent_of("usd"), 2);
        assert_eq!(result.exponent_of("year"), -2);
    }

    #[test]
    fn pow_scalar_stays_scalar() {
        assert!(Unit::scalar().pow(99).is_scalar());
    }

    // ---- Mul<i32> ----

    #[test]
    fn mul_i32_two_is_pow_two() {
        assert_eq!(Unit::single("usd") * 2, Unit::single("usd").pow(2));
    }

    #[test]
    fn mul_i32_zero_is_scalar() {
        assert!((Unit::single("usd") * 0).is_scalar());
    }

    #[test]
    fn mul_i32_neg_one_equals_neg() {
        assert_eq!(Unit::single("usd") * -1, -Unit::single("usd"));
    }

    // ---- MulAssign / DivAssign ----

    #[test]
    fn mulassign_cancels_dim() {
        let mut result = u("usd/sek");
        result *= Unit::single("sek");
        assert_eq!(result, Unit::single("usd"));
    }

    #[test]
    fn divassign_gives_scalar() {
        let mut result = Unit::single("usd");
        result /= Unit::single("usd");
        assert!(result.is_scalar());
    }

    // ---- is_compatible_with ----

    #[test]
    fn compatible_with_same_unit() {
        assert!(Unit::single("usd").is_compatible_with(&Unit::single("usd")));
    }

    #[test]
    fn not_compatible_different_currency() {
        assert!(!Unit::single("usd").is_compatible_with(&Unit::single("sek")));
    }

    #[test]
    fn compatible_with_identical_compound_unit() {
        assert!(u("usd/year").is_compatible_with(&u("usd/year")));
    }

    #[test]
    fn not_compatible_compound_vs_simple() {
        assert!(!u("usd/year").is_compatible_with(&Unit::single("usd")));
    }

    #[test]
    fn compatible_scalars_with_each_other() {
        assert!(Unit::scalar().is_compatible_with(&Unit::scalar()));
    }

    #[test]
    fn not_compatible_scalar_with_typed() {
        assert!(!Unit::scalar().is_compatible_with(&Unit::single("usd")));
    }

    // ---- parse ----

    #[test]
    fn parse_single_dim() {
        assert_eq!(u("usd"), Unit::single("usd"));
    }

    #[test]
    fn parse_ratio_gives_correct_exponents() {
        let unit = u("usd/year");
        assert_eq!(unit.exponent_of("usd"), 1);
        assert_eq!(unit.exponent_of("year"), -1);
    }

    #[test]
    fn parse_currency_ratio() {
        let unit = u("sek/usd");
        assert_eq!(unit.exponent_of("sek"), 1);
        assert_eq!(unit.exponent_of("usd"), -1);
    }

    #[test]
    fn parse_one_and_empty_are_scalar() {
        assert_eq!(Unit::parse("1").unwrap(), Unit::scalar());
        assert_eq!(Unit::parse("").unwrap(), Unit::scalar());
    }

    // ---- Display ----

    #[test]
    fn display_scalar_is_one() {
        assert_eq!(Unit::scalar().to_string(), "1");
    }

    #[test]
    fn display_single_dim() {
        assert_eq!(Unit::single("usd").to_string(), "usd");
    }

    #[test]
    fn display_ratio() {
        assert_eq!(u("usd/year").to_string(), "usd/year");
    }

    #[test]
    fn display_currency_exchange_rate() {
        assert_eq!(u("sek/usd").to_string(), "sek/usd");
    }

    #[test]
    fn display_squared() {
        assert_eq!(Unit::single("usd").pow(2).to_string(), "usd^2");
    }

    #[test]
    fn display_product_alphabetical() {
        // BTreeMap yields sek before usd
        let result = Unit::single("usd") * Unit::single("sek");
        assert_eq!(result.to_string(), "sek*usd");
    }

    #[test]
    fn display_multi_denominator_parenthesised() {
        // usd / (sek * year)
        let result = Unit::single("usd") / (Unit::single("sek") * Unit::single("year"));
        assert_eq!(result.to_string(), "usd/(sek*year)");
    }

    #[test]
    fn display_pure_denominator() {
        // 1/usd
        assert_eq!((-Unit::single("usd")).to_string(), "1/usd");
    }

    // ---- zero-exponent cleanup ----

    #[test]
    fn mul_drops_zero_exponent() {
        let result = Unit::single("usd") * (-Unit::single("usd"));
        assert!(result.is_scalar(), "expected scalar, got: {result:?}");
    }

    #[test]
    fn time_cancellation_drops_zero_exponent() {
        let result = u("usd/year") * Unit::single("year");
        assert_eq!(result, Unit::single("usd"));
        assert_eq!(result.exponent_of("year"), 0);
    }

    #[test]
    fn zero_exponent_not_stored_in_map() {
        let result = Unit::single("usd") / Unit::single("usd");
        assert_eq!(result.dims().count(), 0);
    }
}
