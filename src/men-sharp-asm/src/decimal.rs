//! An exact decimal number, for `System.Decimal` values the emulator holds
//! and for the compiler to normalize a `decimal` literal into the text the
//! Unity importer parses (`decimal.Parse`).
//!
//! `mantissa / 10^scale`, as .NET's decimal is; the scale survives
//! arithmetic the way .NET keeps it (`1.50m + 1m` prints `2.50`), and
//! equality is numeric, so `0.30m == 0.3m`. Division rounds toward zero at
//! whatever precision fits, then drops the trailing zeros, which is close
//! enough to .NET's 28 digits for a test to read.

use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Decimal {
    mantissa: i128,
    scale: u32,
}

impl Decimal {
    pub const ZERO: Decimal = Decimal {
        mantissa: 0,
        scale: 0,
    };

    /// A C# decimal literal without its suffix, or any plain decimal text:
    /// digits, an optional fraction, an optional exponent (`1.5`, `-3`,
    /// `2e3`, `.5`). `None` for anything else or a value too large.
    pub fn parse(text: &str) -> Option<Decimal> {
        let text = text.trim();
        let (negative, rest) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        let (number, exponent) = match rest.find(['e', 'E']) {
            Some(at) => (&rest[..at], rest[at + 1..].parse::<i32>().ok()?),
            None => (rest, 0),
        };
        let (integer, fraction) = match number.split_once('.') {
            Some((integer, fraction)) => (integer, fraction),
            None => (number, ""),
        };
        if integer.is_empty() && fraction.is_empty() {
            return None;
        }
        if !integer.bytes().all(|byte| byte.is_ascii_digit())
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let mut mantissa: i128 = 0;
        for digit in integer.bytes().chain(fraction.bytes()) {
            mantissa = mantissa
                .checked_mul(10)?
                .checked_add(i128::from(digit - b'0'))?;
        }
        let mut scale = i64::from(fraction.len() as u32) - i64::from(exponent);
        while scale < 0 {
            mantissa = mantissa.checked_mul(10)?;
            scale += 1;
        }
        if mantissa == 0 {
            scale = 0;
        }
        if negative {
            mantissa = -mantissa;
        }
        Some(Decimal {
            mantissa,
            scale: u32::try_from(scale).ok()?,
        })
    }

    pub fn from_i64(value: i64) -> Decimal {
        Decimal {
            mantissa: i128::from(value),
            scale: 0,
        }
    }

    /// The shortest decimal that reads back as `value`, as
    /// `Convert.ToDecimal(double)` rounds to what was meant.
    pub fn from_f64(value: f64) -> Option<Decimal> {
        Decimal::parse(&format!("{value}"))
    }

    pub fn to_f64(self) -> f64 {
        self.mantissa as f64 / 10f64.powi(self.scale as i32)
    }

    /// The integral part, toward zero.
    pub fn truncate(self) -> i128 {
        self.mantissa / pow10(self.scale)
    }

    /// Both on one scale: the larger.
    fn aligned(self, other: Decimal) -> Option<(i128, i128, u32)> {
        let scale = self.scale.max(other.scale);
        let a = self
            .mantissa
            .checked_mul(pow10_checked(scale - self.scale)?)?;
        let b = other
            .mantissa
            .checked_mul(pow10_checked(scale - other.scale)?)?;
        Some((a, b, scale))
    }

    pub fn checked_add(self, other: Decimal) -> Option<Decimal> {
        let (a, b, scale) = self.aligned(other)?;
        Some(Decimal {
            mantissa: a.checked_add(b)?,
            scale,
        })
    }

    pub fn checked_sub(self, other: Decimal) -> Option<Decimal> {
        self.checked_add(other.negated())
    }

    pub fn checked_mul(self, other: Decimal) -> Option<Decimal> {
        Some(Decimal {
            mantissa: self.mantissa.checked_mul(other.mantissa)?,
            scale: self.scale + other.scale,
        })
    }

    /// `None` for a zero divisor (DivideByZeroException on .NET).
    pub fn checked_div(self, other: Decimal) -> Option<Decimal> {
        if other.mantissa == 0 {
            return None;
        }
        let (a, b, _) = self.aligned(other)?;
        // as many digits after the point as fit, up to .NET's 28
        let mut extra = 28;
        let scaled = loop {
            if let Some(scaled) = a.checked_mul(pow10_checked(extra)?) {
                break scaled;
            }
            extra -= 1;
        };
        Decimal {
            mantissa: scaled / b,
            scale: extra,
        }
        .without_trailing_zeros()
        .into()
    }

    pub fn checked_rem(self, other: Decimal) -> Option<Decimal> {
        if other.mantissa == 0 {
            return None;
        }
        let (a, b, scale) = self.aligned(other)?;
        Some(Decimal {
            mantissa: a % b,
            scale,
        })
    }

    pub fn negated(self) -> Decimal {
        Decimal {
            mantissa: -self.mantissa,
            scale: self.scale,
        }
    }

    pub fn compare(self, other: Decimal) -> Ordering {
        match self.aligned(other) {
            Some((a, b, _)) => a.cmp(&b),
            None => self.to_f64().total_cmp(&other.to_f64()),
        }
    }

    pub fn equals(self, other: Decimal) -> bool {
        self.compare(other) == Ordering::Equal
    }

    fn without_trailing_zeros(mut self) -> Decimal {
        while self.scale > 0 && self.mantissa % 10 == 0 {
            self.mantissa /= 10;
            self.scale -= 1;
        }
        self
    }
}

fn pow10(exponent: u32) -> i128 {
    10i128.pow(exponent)
}

fn pow10_checked(exponent: u32) -> Option<i128> {
    10i128.checked_pow(exponent)
}

/// As .NET prints it: the scale's digits kept (`1.50`), no exponent.
impl std::fmt::Display for Decimal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let digits = self.mantissa.unsigned_abs().to_string();
        let scale = self.scale as usize;
        let sign = if self.mantissa < 0 { "-" } else { "" };
        if scale == 0 {
            return write!(f, "{sign}{digits}");
        }
        let padded = format!("{digits:0>width$}", width = scale + 1);
        let (integer, fraction) = padded.split_at(padded.len() - scale);
        write!(f, "{sign}{integer}.{fraction}")
    }
}

#[cfg(test)]
mod tests {
    use super::Decimal;

    fn d(text: &str) -> Decimal {
        Decimal::parse(text).unwrap()
    }

    #[test]
    fn literals_keep_their_digits_exactly() {
        assert_eq!(d("0.1").checked_add(d("0.2")).unwrap(), d("0.3"));
        assert!(d("0.1").checked_add(d("0.2")).unwrap().equals(d("0.30")));
        assert_eq!(d("0.1").checked_add(d("0.2")).unwrap().to_string(), "0.3");
        assert_eq!(d("1.50").checked_add(d("1")).unwrap().to_string(), "2.50");
        assert_eq!(d("2e3").to_string(), "2000");
        assert_eq!(d(".5").to_string(), "0.5");
        assert_eq!(d("-0.05").to_string(), "-0.05");
    }

    #[test]
    fn arithmetic() {
        assert_eq!(d("1.5").checked_mul(d("2")).unwrap().to_string(), "3.0");
        assert_eq!(d("1").checked_div(d("4")).unwrap().to_string(), "0.25");
        assert_eq!(d("10").checked_div(d("4")).unwrap().to_string(), "2.5");
        assert_eq!(d("7").checked_rem(d("3")).unwrap().to_string(), "1");
        assert!(d("1").checked_div(d("0")).is_none());
        assert_eq!(d("2.75").truncate(), 2);
        assert_eq!(d("-2.75").truncate(), -2);
        assert!(d("0.3").compare(d("0.25")).is_gt());
        assert_eq!(Decimal::from_f64(0.1).unwrap(), d("0.1"));
    }

    #[test]
    fn what_is_no_decimal_is_refused() {
        assert!(Decimal::parse("").is_none());
        assert!(Decimal::parse("1.2.3").is_none());
        assert!(Decimal::parse("abc").is_none());
        assert!(Decimal::parse("1e999").is_none());
    }
}
