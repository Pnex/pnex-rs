//! Chip-caps ESP8266 — la table de contraintes **silicium** (Brick 0 §1/§2).
//!
//! Niveau « chip » du modèle 3 couches : chip-caps en **code** (ce fichier),
//! overlay board en **data** (`mcu_boards.details`), capability instance en
//! **PG** (`device_capability_instances`).
//!
//! `validate` est le **point unique** de validation des pins : utilisé par le
//! backend à l'admission (`Announce` → provisioning) **et** avant chaque push
//! de commande (POST commands) — une op illégale est rejetée 400 avec raison,
//! jamais poussée au device (brick0.md §6/§8).
//!
//! ESP8266 (Seule cible P0) :
//! - GPIO6–11 = flash SPI → **interdits** ;
//! - GPIO0/2/15 = strapping pins (0/2 = HIGH attendu au boot, 15 = LOW) ;
//! - GPIO16 : pas d'interrupt/PWM, **pulldown only** (pas de pull-up) ;
//! - A0 = canal ADC unique 10-bit (0–1023), aucun GPIO physique — identifiant
//!   fil convenu `A0_GPIO`.
use serde::{Deserialize, Serialize};

use crate::proto::{Mode, ModeOpts, SafeState};

/// Identifiant fil du canal ADC (« A0 ») — **aucun GPIO physique 17** sur
/// l'ESP8266 : c'est l'adresse conventionnelle de l'ADC, utilisée par
/// l'overlay NodeMCU et le firmware ( lecture `analogRead(A0)`).
pub const A0_GPIO: u16 = 17;

/// GPIO strapping « doit être LOW au boot » (GPIO15 = D8).
pub const STRAPPING_LOW: u16 = 15;
/// GPIOs strapping « doivent être HIGH au boot » (GPIO0 = D3, GPIO2 = D4).
pub const STRAPPING_HIGH: [u16; 2] = [0, 2];
/// GPIOs de la flash SPI — interdits en capability (GPIO6–11).
pub const FLASH_PINS: [u16; 6] = [6, 7, 8, 9, 10, 11];
/// GPIO sans pull-up interne (GPIO16 = D0 — pulldown only).
pub const NO_PULLUP: u16 = 16;

/// Pins valides en P0 = tout GPIO 0–16 sauf la flash, plus le canal ADC.
pub fn is_valid_gpio(gpio: u16) -> bool {
    gpio == A0_GPIO || (gpio <= 16 && !FLASH_PINS.contains(&gpio))
}

/// Violation des chip-caps — `reason()` donne le message fil/UI (français,
/// relayé tel quel par le front : convention « erreurs relayées »).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Violation {
    /// GPIO inexistant ou hors adressage ESP8266.
    OutOfRange(u16),
    /// GPIO6–11 (flash SPI).
    FlashPins(u16),
    /// Mode analogique hors A0.
    AnalogOnlyOnA0(u16),
    /// Mode digital sur A0 (l'ADC n'a pas de digital).
    AdcOnlyOnA0(u16),
    /// `safe_state: high` sur GPIO15 — forcerait un boot en mode SD-card.
    StrappingLow(u16),
    /// Pull-up demandée sur GPIO16 (n'existe pas physiquement).
    NoPullUp(u16),
}

impl Violation {
    /// Message fil/UI (relayé tel quel — convention « erreurs relayées »).
    pub fn reason(&self) -> String {
        match self {
            Violation::OutOfRange(g) => format!("gpio {g} : inconnu sur ESP8266"),
            Violation::FlashPins(g) => {
                format!("gpio {g} : GPIO6–11 = flash SPI, interdits en capability")
            }
            Violation::AnalogOnlyOnA0(g) => format!("gpio {g} : analog_in est réservé à A0"),
            Violation::AdcOnlyOnA0(g) => format!("gpio {g} : A0 ne supporte que analog_in"),
            Violation::StrappingLow(g) => {
                format!("gpio {g} : strapping pin boot-LOW, safe_state: high interdit (boot cassé)")
            }
            Violation::NoPullUp(g) => format!("gpio {g} : pas de pull-up interne (pulldown only)"),
        }
    }
}

/// Pin validé — sérialisé tel quel dans `constraints_snapshot` (jsonb) :
/// ce qui a été validé à l'admission, rejouable par l'UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatedPin {
    pub gpio: u16,
    pub mode: Mode,
    /// Pull-up effective (défaut false).
    pub pullup: bool,
    /// Safe-state effective (défaut Low).
    pub safe_state: SafeState,
}

/// Validation d'un pin contre les chip-caps ESP8266 — **point unique** utilisé
/// à l'admission ET avant chaque push de commande (brick0.md §2/§6/§8).
pub fn validate(gpio: u16, mode: Mode, opts: &ModeOpts) -> Result<ValidatedPin, Violation> {
    if !is_valid_gpio(gpio) {
        if FLASH_PINS.contains(&gpio) {
            return Err(Violation::FlashPins(gpio));
        }
        return Err(Violation::OutOfRange(gpio));
    }
    match mode {
        Mode::AdcIn if gpio != A0_GPIO => return Err(Violation::AnalogOnlyOnA0(gpio)),
        m if m != Mode::AdcIn && gpio == A0_GPIO => return Err(Violation::AdcOnlyOnA0(gpio)),
        _ => {}
    }
    let pullup = opts.pullup.unwrap_or(false);
    if pullup && gpio == NO_PULLUP {
        return Err(Violation::NoPullUp(gpio));
    }
    let safe_state = opts.safe_state.unwrap_or(SafeState::Low);
    if safe_state == SafeState::High && gpio == STRAPPING_LOW {
        return Err(Violation::StrappingLow(gpio));
    }
    Ok(ValidatedPin {
        gpio,
        mode,
        pullup,
        safe_state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::ModeOpts;

    fn opts(pullup: Option<bool>, ss: Option<SafeState>) -> ModeOpts {
        ModeOpts {
            pullup,
            safe_state: ss,
        }
    }
    #[test]
    fn regles_chip_caps_esp8266() {
        // flash SPI 6-11 interdits
        let e = validate(8, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::FlashPins(8)));
        assert!(e.reason().contains("flash SPI"));
        // hors plage
        let e = validate(20, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::OutOfRange(20)));
        // analog only sur A0 (17) : digital refusé sur A0, analog refusé ailleurs
        let e = validate(17, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::AdcOnlyOnA0(17)));
        let e = validate(5, Mode::AdcIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::AnalogOnlyOnA0(5)));
        // GPIO15 : safe_state high interdit (strapping boot-LOW)
        let e = validate(15, Mode::DigitalOut, &opts(None, Some(SafeState::High))).unwrap_err();
        assert!(matches!(e, Violation::StrappingLow(15)));
        // GPIO16 : pull-up inexistante
    }
}
