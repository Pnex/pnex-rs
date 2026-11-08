//! Data governance (governance.md D798–D806): markings, clearances, and what a
//! table carries. DTOs and checks, wasm-safe.

use serde::{Deserialize, Serialize};

pub const KEY_MAX: usize = 64;
pub const NAME_MAX: usize = 100;
pub const DESCRIPTION_MAX: usize = 500;
pub const PURPOSE_MAX: usize = 200;
pub const JUSTIFICATION_MAX: usize = 500;
/// Longest clearance, in days.
pub const CLEARANCE_DAYS_MAX: i64 = 365;

/// A compartment of the org: seeing data that carries it needs a clearance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Marking {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub key: String,
    pub name: String,
    pub description: String,
    /// Tables and objects that carry it: a carried marking cannot be deleted.
    pub uses: i64,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MarkingInput {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// A key in English: lowercase letters, digits, `_`.
pub fn key_ok(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= KEY_MAX
        && k.starts_with(|c: char| c.is_ascii_lowercase())
        && k.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl MarkingInput {
    pub fn violation(&self) -> Option<(&'static str, String)> {
        if !key_ok(&self.key) {
            return Some(("key", "invalid".into()));
        }
        if self.name.trim().is_empty() {
            return Some(("name", "required".into()));
        }
        if self.name.chars().count() > NAME_MAX {
            return Some(("name", format!("max_length:{NAME_MAX}")));
        }
        (self.description.chars().count() > DESCRIPTION_MAX)
            .then(|| ("description", format!("max_length:{DESCRIPTION_MAX}")))
    }
}

/// A person cleared for a marking, for a purpose, until a date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Clearance {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    #[cfg_attr(feature = "openapi", schema(format = "int64"))]
    pub user_id: i64,
    pub user_email: String,
    pub marking_key: String,
    pub purpose: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub expires_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub revoked_at: Option<String>,
    /// Neither revoked nor expired.
    pub active: bool,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ClearanceInput {
    #[cfg_attr(feature = "openapi", schema(format = "int64"))]
    pub user_id: i64,
    pub marking_key: String,
    pub purpose: String,
    /// Days from now, 1 to 365.
    pub days: i64,
}

impl ClearanceInput {
    pub fn violation(&self) -> Option<(&'static str, String)> {
        if !key_ok(&self.marking_key) {
            return Some(("marking_key", "invalid".into()));
        }
        if self.purpose.trim().is_empty() {
            return Some(("purpose", "required".into()));
        }
        if self.purpose.chars().count() > PURPOSE_MAX {
            return Some(("purpose", format!("max_length:{PURPOSE_MAX}")));
        }
        (!(1..=CLEARANCE_DAYS_MAX).contains(&self.days))
            .then(|| ("days", format!("range:1:{CLEARANCE_DAYS_MAX}")))
    }
}

/// Markings added to a table; removing one is a declassification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MarkingsAdd {
    pub markings: Vec<String>,
}

/// Removes one marking, for a reason kept in the journal (D802).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Declassify {
    pub marking: String,
    pub justification: String,
}

impl Declassify {
    pub fn violation(&self) -> Option<(&'static str, String)> {
        if !key_ok(&self.marking) {
            return Some(("marking", "invalid".into()));
        }
        if self.justification.trim().chars().count() < 10 {
            return Some(("justification", "min_length:10".into()));
        }
        (self.justification.chars().count() > JUSTIFICATION_MAX)
            .then(|| ("justification", format!("max_length:{JUSTIFICATION_MAX}")))
    }
}

/// What the detection recognizes in a column (D804).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensitive {
    Email,
    Phone,
    Iban,
    Card,
    Nir,
}

impl Sensitive {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Phone => "phone",
            Self::Iban => "iban",
            Self::Card => "card",
            Self::Nir => "nir",
        }
    }

    fn matches(self, v: &str) -> bool {
        let digits: String = v.chars().filter(char::is_ascii_digit).collect();
        match self {
            Self::Email => v.split_once('@').is_some_and(|(l, d)| {
                !l.is_empty() && d.contains('.') && !d.starts_with('.') && !v.contains(' ')
            }),
            Self::Phone => {
                v.chars()
                    .all(|c| c.is_ascii_digit() || " +.-()".contains(c))
                    && (9..=15).contains(&digits.len())
                    && (v.starts_with('+') || v.starts_with('0'))
            }
            Self::Iban => iban_ok(v),
            Self::Card => {
                v.chars()
                    .all(|c| c.is_ascii_digit() || c == ' ' || c == '-')
                    && (13..=19).contains(&digits.len())
                    && luhn(&digits)
            }
            Self::Nir => nir_ok(&v.replace(' ', "")),
        }
    }
}

fn luhn(digits: &str) -> bool {
    let sum: u32 = digits
        .chars()
        .rev()
        .filter_map(|c| c.to_digit(10))
        .enumerate()
        .map(|(i, d)| {
            if i % 2 == 1 {
                if d * 2 > 9 {
                    d * 2 - 9
                } else {
                    d * 2
                }
            } else {
                d
            }
        })
        .sum();
    sum % 10 == 0
}

fn mod97(s: &str) -> Option<u32> {
    s.chars().try_fold(0u32, |acc, c| {
        let n = c.to_digit(36)?;
        Some(if n > 9 {
            (acc * 100 + n) % 97
        } else {
            (acc * 10 + n) % 97
        })
    })
}

fn iban_ok(v: &str) -> bool {
    let s: String = v
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    if !(15..=34).contains(&s.len())
        || !s[..2].chars().all(|c| c.is_ascii_uppercase())
        || !s.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return false;
    }
    mod97(&format!("{}{}", &s[4..], &s[..4])) == Some(1)
}

/// A French social security number: 13 digits (Corsica: 2A/2B) and a key of 2.
fn nir_ok(s: &str) -> bool {
    if s.len() != 15 || !matches!(s.as_bytes()[0], b'1' | b'2') {
        return false;
    }
    let body = s[..13].replace("2A", "19").replace("2B", "18");
    let (Ok(n), Ok(key)) = (body.parse::<u64>(), s[13..].parse::<u64>()) else {
        return false;
    };
    97 - n % 97 == key
}

/// Values of a column to judge: at least this many non-empty ones.
pub const DETECT_MIN_VALUES: usize = 5;
/// Share of non-empty values that must match, in percent.
pub const DETECT_SHARE: u32 = 60;

/// The category most values of a column match, with its share in percent.
pub fn detect<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<(Sensitive, u32)> {
    let values: Vec<&str> = values
        .into_iter()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect();
    if values.len() < DETECT_MIN_VALUES {
        return None;
    }
    [
        Sensitive::Iban,
        Sensitive::Card,
        Sensitive::Nir,
        Sensitive::Email,
        Sensitive::Phone,
    ]
    .into_iter()
    .map(|c| {
        (
            c,
            (values.iter().filter(|v| c.matches(v)).count() * 100 / values.len()) as u32,
        )
    })
    .filter(|(_, share)| *share >= DETECT_SHARE)
    .max_by_key(|(_, share)| *share)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_are_checked() {
        let m = MarkingInput {
            key: "hr".into(),
            name: "HR".into(),
            description: String::new(),
        };
        assert!(m.violation().is_none());
        for bad in ["", "HR", "1hr", "hr-data", "h r"] {
            let b = MarkingInput {
                key: bad.into(),
                ..m.clone()
            };
            assert_eq!(b.violation().unwrap().0, "key", "{bad}");
        }
        let c = ClearanceInput {
            user_id: 1,
            marking_key: "hr".into(),
            purpose: "payroll 2026".into(),
            days: 30,
        };
        assert!(c.violation().is_none());
        assert_eq!(
            ClearanceInput {
                days: 0,
                ..c.clone()
            }
            .violation()
            .unwrap()
            .0,
            "days"
        );
        assert_eq!(
            ClearanceInput {
                days: 366,
                ..c.clone()
            }
            .violation()
            .unwrap()
            .0,
            "days"
        );
        assert_eq!(
            ClearanceInput {
                purpose: " ".into(),
                ..c
            }
            .violation()
            .unwrap()
            .0,
            "purpose"
        );
        let d = Declassify {
            marking: "hr".into(),
            justification: "too short".into(),
        };
        assert_eq!(d.violation().unwrap().0, "justification");
    }

    #[test]
    fn sensitive_columns_are_recognized() {
        let col = |v: &[&'static str]| detect(v.iter().copied()).map(|(c, _)| c);
        assert_eq!(
            col(&[
                "ada@example.org",
                "bob@x.fr",
                "c@d.io",
                "e@f.com",
                "g@h.net"
            ]),
            Some(Sensitive::Email)
        );
        assert_eq!(
            col(&[
                "+33 6 12 34 56 78",
                "0612345678",
                "01.23.45.67.89",
                "+44 20 7946 0958",
                "0698765432"
            ]),
            Some(Sensitive::Phone)
        );
        // Valid check digits only: the same shape with a wrong key is not an IBAN.
        let ibans = [
            "FR14 2004 1010 0505 0001 3M02 606",
            "DE89370400440532013000",
            "GB82WEST12345698765432",
            "FR7630006000011234567890189",
            "NL91ABNA0417164300",
        ];
        assert_eq!(col(&ibans), Some(Sensitive::Iban));
        assert!(!iban_ok("FR15 2004 1010 0505 0001 3M02 606"));
        assert_eq!(
            col(&[
                "4111 1111 1111 1111",
                "5500-0000-0000-0004",
                "340000000000009",
                "4012888888881881",
                "6011000000000004"
            ]),
            Some(Sensitive::Card)
        );
        assert!(!luhn("4111111111111112"));
        assert!(nir_ok("255081416802538"));
        assert!(!nir_ok("255081416802539"));
        // Ordinary data stays unflagged, and too few values say nothing.
        assert_eq!(col(&["Lyon", "Nantes", "Brest", "Paris", "Lille"]), None);
        assert_eq!(col(&["12", "15", "18", "21", "24"]), None);
        assert_eq!(col(&["ada@example.org", "bob@x.fr"]), None);
        assert_eq!(
            col(&["ada@example.org", "b@x.fr", "Lyon", "Paris", "Lille"]),
            None
        );
    }
}
