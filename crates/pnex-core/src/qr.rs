//! QR tags (parts-docs.md D962–D963): a printed QR holds a short address
//! to a token, never data in clear. The token is random (not enumerable),
//! belongs to one org, and is blank (printed ahead on a sheet), linked to
//! one object of any type, or revoked.

use serde::{Deserialize, Serialize};

/// Tags of one blank sheet at most (three A4 pages of 40 labels).
pub const SHEET_MAX: u32 = 120;
/// Length of a token: 26 base32 characters (130 bits).
pub const TOKEN_LEN: usize = 26;
const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum QrState {
    Blank,
    Linked,
    Revoked,
}

impl QrState {
    pub fn as_str(self) -> &'static str {
        match self {
            QrState::Blank => "blank",
            QrState::Linked => "linked",
            QrState::Revoked => "revoked",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [QrState::Blank, QrState::Linked, QrState::Revoked]
            .into_iter()
            .find(|k| k.as_str() == s)
    }
}

/// A tag as read by a member of its org.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct QrTag {
    pub token: String,
    pub state: QrState,
    /// The object a linked tag opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub object_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_type: Option<String>,
    /// The blank sheet it was printed on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub sheet_id: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub linked_at: Option<String>,
}

/// `POST /qr-tags/sheets`: blank tags to print.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct QrSheetInput {
    /// 1 to [`SHEET_MAX`].
    pub count: u32,
}

/// A blank sheet just made, its tags in printing order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct QrSheet {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub sheet_id: String,
    pub tags: Vec<QrTag>,
}

/// `POST /qr-tags/{token}/link`: the object a blank tag now opens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct QrLinkInput {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub object_id: String,
}

/// A new token from 26 random bytes (only the low 5 bits of each are used).
pub fn token_from(random: &[u8; TOKEN_LEN]) -> String {
    random
        .iter()
        .map(|b| ALPHABET[(b & 31) as usize] as char)
        .collect()
}

/// Whether `s` has the shape of a token (checked before any database read).
pub fn valid_token(s: &str) -> bool {
    s.len() == TOKEN_LEN && s.bytes().all(|b| ALPHABET.contains(&b))
}

/// The address printed in a QR: `<origin>/q/<token>`.
pub fn address(origin: &str, token: &str) -> String {
    format!("{}/q/{token}", origin.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_have_one_shape() {
        let t = token_from(&[255; TOKEN_LEN]);
        assert_eq!(t, "7".repeat(TOKEN_LEN));
        assert!(valid_token(&t));
        assert!(!valid_token("short"));
        assert!(!valid_token(&"A".repeat(TOKEN_LEN)));
        assert!(!valid_token(&format!("{}/", "a".repeat(TOKEN_LEN - 1))));
        assert_eq!(
            address("https://pnex.test/", "abc"),
            "https://pnex.test/q/abc"
        );
        assert_eq!(QrState::parse("linked"), Some(QrState::Linked));
    }
}
