//! Overlays board — le niveau **data** du modèle 3 couches (Brick 0 §1).
//!
//! Un overlay décrit le câblage d'une carte (labels D0…D8/A0 → GPIO). Il vit
//! en `mcu_boards.details` (jsonb, jamais en `.h` — §2.3 du PRD) et se
//! désérialise en `BoardOverlay` côté serveur pour dériver la carte de pins
//! à l'admission. Contribuable en data (fixture YAML) sans recompilation.

use serde::{Deserialize, Serialize};

/// Carte de pins d'une board — contenu de `mcu_boards.details`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardOverlay {
    /// Identifiant de board (« nodemcu », « d1_mini »…).
    pub board: String,
    /// Pins exposés à l'utilisateur.
    pub pins: Vec<BoardPin>,
}

/// Un pin d'overlay.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BoardPin {
    /// Label overlay (« D1 », « A0 »).
    pub label: String,
    pub gpio: u16,
    /// digital | analog — un pin analog ne propose que `analog_in`.
    pub kind: PinKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinKind {
    Digital,
    Analog,
}

impl BoardOverlay {
    /// Cherche un pin par GPIO.
    pub fn pin_by_gpio(&self, gpio: u16) -> Option<BoardPin> {
        self.pins.iter().find(|p| p.gpio == gpio).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_parse_depuis_details_jsonb() {
        let json = r#"{"board":"nodemcu","pins":[{"label":"D1","gpio":5,"kind":"digital"},{"label":"A0","gpio":17,"kind":"analog"}]}"#;
        let o: BoardOverlay = serde_json::from_str(json).unwrap();
        assert_eq!(o.board, "nodemcu");
        assert_eq!(o.pins.len(), 2);
        assert_eq!(o.pin_by_gpio(5).unwrap().label, "D1");
    }
}
