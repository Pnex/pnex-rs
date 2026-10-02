//! Chip-caps par SoC — la table de contraintes **silicium** (Brick 0 §1/§2).
//!
//! Niveau « chip » du modèle 3 couches : chip-caps en **code** (ce fichier),
//! overlay board en **data** (`mcu_boards.details`), capability instance en
//! **PG** (`device_capability_instances`).
//!
//! `validate` est le **point unique** de validation des pins : utilisé par le
//! backend à l'admission (`Announce` → provisioning), avant chaque push de
//! commande (POST commands) et au cast des cartes régulation (regulator.rs)
//! — une op illégale est rejetée 400 avec raison, jamais poussée au device
//! (brick0.md §6/§8).
//!
//! ESP8266 (P0) :
//! - GPIO6–11 = flash SPI → **interdits** ;
//! - GPIO0/2/15 = strapping pins (0/2 = HIGH attendu au boot, 15 = LOW) ;
//! - GPIO16 : pas d'interrupt/PWM, **pulldown only** (pas de pull-up) ;
//! - A0 = canal ADC unique 10-bit (0–1023), aucun GPIO physique — identifiant
//!   fil convenu `A0_GPIO`.
//!
//! ESP32-C3 (P1, Seeed XIAO) :
//! - GPIO11–17 = flash SPI interne → **interdits** ;
//! - GPIO18/19 = USB D-/D+ (USB-JTAG/serial = le port de flash/monitor) →
//!   **réservés** ;
//! - GPIO2/8/9 = strapping boot-HIGH (pull-ups externes XIAO) — `safe_state:
//!   low` interdit en digital_out (risque de boot en download mode) ;
//! - analog_in limité aux GPIO0–4 (ADC1) — GPIO5 = ADC2, inutilisable avec
//!   le WiFi actif.
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

// ─────────────────────── ESP32-C3 ───────────────────────

/// Flash SPI interne (GPIO11–17) — interdits en capability (esp32-c3).
pub const C3_FLASH_PINS: [u16; 7] = [11, 12, 13, 14, 15, 16, 17];
/// USB D-/D+ (GPIO18/19) — réservés : ce sont le port de flash/monitor USB
/// (USB-JTAG/serial). Une capability dessus tuerait l'accès au device.
pub const C3_USB_PINS: [u16; 2] = [18, 19];
/// Strapping boot-HIGH (GPIO2/8/9) — pull-ups externes XIAO ; forcer LOW au
/// boot = risque de download mode.
pub const C3_STRAPPING_HIGH: [u16; 3] = [2, 8, 9];
/// ADC1 (GPIO0–4) — seuls canaux ADC utilisables avec le WiFi actif
/// (GPIO5 = ADC2, cassé avec WiFi actif).
pub const C3_ADC1_PINS: [u16; 5] = [0, 1, 2, 3, 4];

// ─────────────────────── ESP32 (classique) ───────────────────────
// Custom firmware on a classic ESP32 (pio board `esp32dev`) — same rule
// grid as the C3: protected strapping, ADC1 only, input-only without pull-up.

/// Flash SPI (GPIO6–11) — interdits en capability (esp32 classique).
pub const ESP32_FLASH_PINS: [u16; 6] = [6, 7, 8, 9, 10, 11];
/// GPIOs inexistants sur l'ESP32 classique (adressage troué).
const ESP32_ABSENT_PINS: [u16; 6] = [20, 24, 28, 29, 30, 31];
/// Strapping boot-HIGH (GPIO0, GPIO15 — doivent être HIGH/flottant au
/// boot) : `safe_state: low` interdit en digital_out.
pub const ESP32_STRAPPING_HIGH: [u16; 2] = [0, 15];
/// Strapping boot-LOW (GPIO12 = MTDI — tension flash ; GPIO2 = requis bas
/// en download mode) : `safe_state: high` interdit en digital_out.
pub const ESP32_STRAPPING_LOW: [u16; 2] = [2, 12];
/// Input-only (GPIO34–39) : pas de sortie, pas de pull-up interne.
pub const ESP32_INPUT_ONLY: [u16; 6] = [34, 35, 36, 37, 38, 39];
/// ADC1 (GPIO32–39) — seuls canaux utilisables avec le WiFi actif (ADC2
/// cassé avec WiFi). 37/38 occupés par la PSRAM sur WROVER, libres en
/// WROOM.
pub const ESP32_ADC1_PINS: [u16; 8] = [32, 33, 34, 35, 36, 37, 38, 39];

// ─────────────────────── ESP32-S3 ───────────────────────
// P2 (DevKitC-1, pio board `esp32-s3-devkitc-1`). Full GPIO matrix: no
// dedicated I2C/SPI pins — Arduino conventions chosen for the screen
// standards (Wire 8/9, FSPI 10-13).

/// Internal flash SPI (GPIO26–32) — forbidden for capabilities (esp32-s3).
pub const S3_FLASH_PINS: [u16; 7] = [26, 27, 28, 29, 30, 31, 32];
/// Octal PSRAM (GPIO33–37) — conservatively forbidden: wiring present on
/// N8R8 modules (never a capability, whatever the module).
pub const S3_PSRAM_PINS: [u16; 5] = [33, 34, 35, 36, 37];
/// USB D-/D+ (GPIO19/20) — reserved: USB-JTAG/serial flash/monitor port.
pub const S3_USB_PINS: [u16; 2] = [19, 20];
/// Strapping boot-HIGH (GPIO0/3 = boot mode; GPIO45/46 = flash voltage /
/// boot): `safe_state: low` forbidden on outputs (broken boot risk).
pub const S3_STRAPPING_HIGH: [u16; 4] = [0, 3, 45, 46];
/// GPIO46: the only input-only pin on the S3 (weak internal pull-down, no
/// pull-up, no output drive).
pub const S3_INPUT_ONLY: [u16; 1] = [46];
/// ADC1 (GPIO1–10) — only channels usable with WiFi active (ADC2 =
/// GPIO11–20, broken with WiFi).
pub const S3_ADC1_PINS: [u16; 10] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

/// SoC reconnus par les chip-caps — dérivé de `mcu_boards.soc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Soc {
    Esp8266,
    Esp32C3,
    Esp32,
    Esp32S3,
}

impl Soc {
    /// Convention `mcu_boards.soc` / chip announce (« esp8266 »,
    /// « esp32-c3 », « esp32 », « esp32-s3 ») → SoC. Unknown → `None`: the
    /// call-sites fail closed (never a silent validation against another
    /// SoC's rules).
    pub fn from_board_soc(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "esp8266" => Some(Soc::Esp8266),
            "esp32-c3" | "esp32c3" => Some(Soc::Esp32C3),
            "esp32" => Some(Soc::Esp32),
            "esp32-s3" | "esp32s3" => Some(Soc::Esp32S3),
            _ => None,
        }
    }

    /// Nom fil canonique (« esp8266 », « esp32-c3 », « esp32 ») — chip
    /// annoncé par la lib PneX et valeur persistée dans
    /// `device_registries.soc` (SoC observed at the first announce).
    pub fn name(self) -> &'static str {
        match self {
            Soc::Esp8266 => "esp8266",
            Soc::Esp32C3 => "esp32-c3",
            Soc::Esp32 => "esp32",
            Soc::Esp32S3 => "esp32-s3",
        }
    }
}

/// Valid esp32-s3 pins = GPIO0–18, 21, 38–48 (flash 26–32 and PSRAM 33–37
/// forbidden, USB 19/20 reserved).
pub fn is_valid_gpio_esp32s3(gpio: u16) -> bool {
    gpio <= 48
        && !S3_FLASH_PINS.contains(&gpio)
        && !S3_PSRAM_PINS.contains(&gpio)
        && !S3_USB_PINS.contains(&gpio)
        && !(22..=25).contains(&gpio)
}

/// UART0 console pins taken by default by the generic firmware (`Serial`
/// logs + flash/monitor bridge): GPIO1 (TX) / GPIO3 (RX) on ESP8266 and
/// ESP32 classic. Configuring them as gpio silences the serial console, so
/// they are never user capabilities (rejected by `validate`, skipped at
/// admission). ESP32-C3/S3 log over native USB-CDC: their UART0 stays free.
pub fn is_console_pin(soc: Soc, gpio: u16) -> bool {
    matches!(soc, Soc::Esp8266 | Soc::Esp32) && (gpio == 1 || gpio == 3)
}

/// Pins valides en P0 (esp8266) = tout GPIO 0–16 sauf la flash, plus le canal ADC.
pub fn is_valid_gpio(gpio: u16) -> bool {
    gpio == A0_GPIO || (gpio <= 16 && !FLASH_PINS.contains(&gpio))
}

/// Pins valides esp32-c3 = GPIO0–10, 20, 21 (la flash 11–17 et l'USB 18/19
/// sont réservés).
pub fn is_valid_gpio_esp32c3(gpio: u16) -> bool {
    gpio <= 21 && !C3_FLASH_PINS.contains(&gpio) && !C3_USB_PINS.contains(&gpio)
}

/// Violation des chip-caps — `reason()` donne le message fil/UI (français,
/// relayé tel quel par le front : convention « erreurs relayées »).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Violation {
    /// GPIO inexistant ou hors adressage ESP8266.
    OutOfRange(u16),
    /// GPIO6–11 (flash SPI).
    FlashPins(u16),
    /// Octal PSRAM pins (esp32-s3: GPIO33–37).
    PsramPins(u16),
    /// USB D-/D+ (esp32-c3 : 18/19) — réservés au port USB-JTAG/serial.
    UsbPins(u16),
    /// UART0 console pins kept by the generic firmware's `Serial` (see
    /// [`is_console_pin`]) — never user capabilities.
    ConsolePins(u16),
    /// analog_in hors ADC1 (esp32-c3 : GPIO5 = ADC2, cassé avec WiFi).
    Adc2Wifi(u16),
    /// Mode analogique hors A0.
    AnalogOnlyOnA0(u16),
    /// Mode digital sur A0 (l'ADC n'a pas de digital).
    AdcOnlyOnA0(u16),
    /// `safe_state: high` sur GPIO15 — forcerait un boot en mode SD-card.
    StrappingLow(u16),
    /// `safe_state: low` sur strapping boot-HIGH (esp32-c3 : GPIO2/8/9) —
    /// risque de boot en download mode.
    StrappingHigh(u16),
    /// Sortie demandée sur un pin input-only (esp32 : GPIO34–39).
    InputOnlyPin(u16),
    /// Pull-up demandée sur GPIO16 (n'existe pas physiquement).
    NoPullUp(u16),
    /// pwm_out demandé sur un GPIO sans timer waveform (esp8266 : GPIO16).
    PwmUnsupportedPin(u16),
}

impl Violation {
    /// Message fil/UI (relayé tel quel — convention « erreurs relayées »).
    /// Canonical English display text (fallback; gpio embedded for
    /// non-i18n consumers).
    pub fn reason(&self) -> String {
        match self {
            Violation::OutOfRange(g) => format!("gpio {g}: unknown on ESP8266"),
            Violation::FlashPins(g) => {
                format!("gpio {g}: flash SPI pins, forbidden as capability")
            }
            Violation::PsramPins(g) => {
                format!("gpio {g}: octal PSRAM (N8R8 module), forbidden as capability")
            }
            Violation::UsbPins(g) => {
                format!("gpio {g}: USB D-/D+ reserved for the USB-JTAG/serial port (flash/monitor)")
            }
            Violation::ConsolePins(g) => {
                format!("gpio {g}: UART0 console (TX/RX) reserved for the serial monitor")
            }
            Violation::Adc2Wifi(g) => {
                format!("gpio {g}: ADC2 unusable with WiFi active — analog_in limited to ADC1")
            }
            Violation::AnalogOnlyOnA0(g) => format!("gpio {g}: analog_in is reserved to A0"),
            Violation::AdcOnlyOnA0(g) => format!("gpio {g}: A0 only supports analog_in"),
            Violation::StrappingLow(g) => {
                format!(
                    "gpio {g}: strapping pin boot-LOW, safe_state: high forbidden (broken boot)"
                )
            }
            Violation::StrappingHigh(g) => {
                format!(
                    "gpio {g}: strapping pin boot-HIGH, safe_state: low forbidden (risk of boot into download mode)"
                )
            }
            Violation::InputOnlyPin(g) => {
                format!("gpio {g}: input-only — output impossible (no internal driver)")
            }
            Violation::NoPullUp(g) => format!("gpio {g}: no internal pull-up (pulldown only)"),
            Violation::PwmUnsupportedPin(g) => {
                format!("gpio {g}: pwm_out impossible (no waveform timer)")
            }
        }
    }

    /// Machine code (kebab, `caps-` prefixed) — the frontend resolves
    /// `err-<code>` with the gpio as `$value`.
    pub fn code(&self) -> &'static str {
        match self {
            Violation::OutOfRange(_) => "caps-out-of-range",
            Violation::FlashPins(_) => "caps-flash-pins",
            Violation::PsramPins(_) => "caps-psram-pins",
            Violation::UsbPins(_) => "caps-usb-pins",
            Violation::ConsolePins(_) => "caps-console-pins",
            Violation::Adc2Wifi(_) => "caps-adc2-wifi",
            Violation::AnalogOnlyOnA0(_) => "caps-analog-only-on-a0",
            Violation::AdcOnlyOnA0(_) => "caps-adc-only-on-a0",
            Violation::StrappingLow(_) => "caps-strapping-low",
            Violation::StrappingHigh(_) => "caps-strapping-high",
            Violation::InputOnlyPin(_) => "caps-input-only-pin",
            Violation::NoPullUp(_) => "caps-no-pull-up",
            Violation::PwmUnsupportedPin(_) => "caps-pwm-unsupported-pin",
        }
    }

    /// Machine token served in display payloads (`"caps-flash-pins:12"`):
    /// the `:gpio` suffix becomes the `$value` fluent arg at resolution.
    pub fn token(&self) -> String {
        match self {
            Violation::OutOfRange(g) => format!("caps-out-of-range:{g}"),
            Violation::FlashPins(g) => format!("caps-flash-pins:{g}"),
            Violation::PsramPins(g) => format!("caps-psram-pins:{g}"),
            Violation::UsbPins(g) => format!("caps-usb-pins:{g}"),
            Violation::ConsolePins(g) => format!("caps-console-pins:{g}"),
            Violation::Adc2Wifi(g) => format!("caps-adc2-wifi:{g}"),
            Violation::AnalogOnlyOnA0(g) => format!("caps-analog-only-on-a0:{g}"),
            Violation::AdcOnlyOnA0(g) => format!("caps-adc-only-on-a0:{g}"),
            Violation::StrappingLow(g) => format!("caps-strapping-low:{g}"),
            Violation::StrappingHigh(g) => format!("caps-strapping-high:{g}"),
            Violation::InputOnlyPin(g) => format!("caps-input-only-pin:{g}"),
            Violation::NoPullUp(g) => format!("caps-no-pull-up:{g}"),
            Violation::PwmUnsupportedPin(g) => format!("caps-pwm-unsupported-pin:{g}"),
        }
    }
}

// ───────────────── Display-only derived API (UI pinout editor) ─────────────────
//
// Everything below is PRESENTATION data derived from the silicon tables
// above — never a validation source. `validate` stays the single gate;
// the frontend renders what these functions return without ever
// re-implementing a rule.

/// Pin mux function, display-only (legend, color bands, sub-labels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinFn {
    Adc1,
    Adc2,
    Dac,
    Touch,
    I2cSda,
    I2cScl,
    SpiMosi,
    SpiMiso,
    SpiClk,
    SpiCs,
    UartTx,
    UartRx,
    Led,
}

/// Pin flag, display-only — derived from the tables above, no new rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinFlag {
    InputOnly,
    Strapping,
    /// Serial console / flash / USB wiring.
    Reserved,
}

/// Display-only mux functions of a gpio (`[]` = plain gpio).
pub fn pin_fns(soc: Soc, gpio: u16) -> &'static [PinFn] {
    const NONE: &[PinFn] = &[];
    match soc {
        Soc::Esp8266 => match gpio {
            A0_GPIO => &[PinFn::Adc1],
            1 => &[PinFn::UartTx],
            3 => &[PinFn::UartRx],
            4 => &[PinFn::I2cSda],
            5 => &[PinFn::I2cScl],
            14 => &[PinFn::SpiClk],
            12 => &[PinFn::SpiMiso],
            13 => &[PinFn::SpiMosi],
            15 => &[PinFn::SpiCs],
            2 => &[PinFn::Led],
            _ => NONE,
        },
        Soc::Esp32C3 => match gpio {
            0 | 1 | 2 | 3 | 4 => &[PinFn::Adc1],
            5 => &[PinFn::Adc2],
            6 => &[PinFn::I2cSda],
            7 => &[PinFn::I2cScl],
            8 => &[PinFn::SpiClk],
            9 => &[PinFn::SpiMiso],
            10 => &[PinFn::SpiMosi],
            20 => &[PinFn::UartRx],
            21 => &[PinFn::UartTx],
            _ => NONE,
        },
        Soc::Esp32 => match gpio {
            32 | 33 => &[PinFn::Adc1, PinFn::Touch],
            34 | 35 | 36 | 37 | 38 | 39 => &[PinFn::Adc1],
            25 | 26 => &[PinFn::Adc2, PinFn::Dac],
            0 | 4 | 12 | 13 | 14 | 15 | 27 => &[PinFn::Adc2, PinFn::Touch],
            2 => &[PinFn::Adc2, PinFn::Touch, PinFn::Led],
            23 => &[PinFn::SpiMosi],
            19 => &[PinFn::SpiMiso],
            18 => &[PinFn::SpiClk],
            5 => &[PinFn::SpiCs],
            21 => &[PinFn::I2cSda],
            22 => &[PinFn::I2cScl],
            1 | 17 => &[PinFn::UartTx],
            3 | 16 => &[PinFn::UartRx],
            _ => NONE,
        },
        // Arduino conventions (Wire 8/9, FSPI 10–13) — the S3 routes any
        // peripheral to any pin, these are only the display hints.
        Soc::Esp32S3 => match gpio {
            1 | 2 | 3 | 4 | 5 | 6 | 7 => &[PinFn::Adc1],
            8 => &[PinFn::Adc1, PinFn::I2cSda],
            9 => &[PinFn::Adc1, PinFn::I2cScl],
            10 => &[PinFn::Adc1, PinFn::SpiCs],
            11 => &[PinFn::Adc2, PinFn::SpiMosi],
            12 => &[PinFn::Adc2, PinFn::SpiClk],
            13 => &[PinFn::Adc2, PinFn::SpiMiso],
            14 | 15 | 16 | 17 | 18 | 19 | 20 => &[PinFn::Adc2],
            43 => &[PinFn::UartTx],
            44 => &[PinFn::UartRx],
            _ => NONE,
        },
    }
}

/// Display-only flags of a gpio (`[]` = no caveat).
pub fn pin_flags(soc: Soc, gpio: u16) -> &'static [PinFlag] {
    const NONE: &[PinFlag] = &[];
    match soc {
        Soc::Esp8266 => match gpio {
            A0_GPIO => &[PinFlag::InputOnly],
            STRAPPING_LOW | 0 | 2 => &[PinFlag::Strapping],
            1 | 3 => &[PinFlag::Reserved],
            _ => NONE,
        },
        Soc::Esp32C3 => match gpio {
            2 | 8 | 9 => &[PinFlag::Strapping],
            18 | 19 => &[PinFlag::Reserved],
            _ => NONE,
        },
        Soc::Esp32 => match gpio {
            34 | 35 | 36 | 37 | 38 | 39 => &[PinFlag::InputOnly],
            0 | 15 | 2 | 12 => &[PinFlag::Strapping],
            1 | 3 => &[PinFlag::Reserved],
            _ => NONE,
        },
        Soc::Esp32S3 => match gpio {
            46 => &[PinFlag::Strapping, PinFlag::InputOnly],
            0 | 3 | 45 => &[PinFlag::Strapping],
            19 | 20 | 43 | 44 => &[PinFlag::Reserved],
            _ => NONE,
        },
    }
}

/// Modes the UI may offer for a gpio, in display order (digital_in,
/// digital_out, pwm_out, analog_in). Structural for outputs: a
/// strapping-high pin IS a legal output with `safe_state: high`, and
/// validate() with default opts would wrongly drop it.
pub fn available_modes(soc: Soc, gpio: u16) -> Vec<Mode> {
    let mut modes = Vec::with_capacity(4);
    if is_console_pin(soc, gpio) {
        return modes;
    }
    if validate(soc, gpio, Mode::DigitalIn, &ModeOpts::default()).is_ok() {
        modes.push(Mode::DigitalIn);
    }
    let output_capable = match soc {
        Soc::Esp8266 => is_valid_gpio(gpio) && gpio != A0_GPIO,
        Soc::Esp32C3 => is_valid_gpio_esp32c3(gpio),
        Soc::Esp32 => is_valid_gpio_esp32(gpio) && !ESP32_INPUT_ONLY.contains(&gpio),
        Soc::Esp32S3 => is_valid_gpio_esp32s3(gpio) && !S3_INPUT_ONLY.contains(&gpio),
    };
    if output_capable {
        modes.push(Mode::DigitalOut);
        // esp8266 GPIO16 has no waveform timer.
        if !(soc == Soc::Esp8266 && gpio == NO_PULLUP) {
            modes.push(Mode::PwmOut);
        }
    }
    if validate(soc, gpio, Mode::AdcIn, &ModeOpts::default()).is_ok() {
        modes.push(Mode::AdcIn);
    }
    modes
}

/// Display-only warnings (`Violation::token()` machine tokens, resolved to
/// fluent keys client-side) shown by the pinout editor — the enforcement
/// stays in `validate` at command time.
pub fn pin_warnings(soc: Soc, gpio: u16) -> Vec<String> {
    let mut w = Vec::new();
    match soc {
        Soc::Esp8266 => {
            if gpio == A0_GPIO {
                return w; // convention ADC : rien à signaler
            }
            if FLASH_PINS.contains(&gpio) {
                w.push(Violation::FlashPins(gpio).token());
            }
            if gpio == STRAPPING_LOW {
                w.push(Violation::StrappingLow(gpio).token());
            }
            if STRAPPING_HIGH.contains(&gpio) {
                w.push(Violation::StrappingHigh(gpio).token());
            }
            if is_console_pin(soc, gpio) {
                w.push(Violation::ConsolePins(gpio).token());
            }
        }
        Soc::Esp32C3 => {
            if C3_FLASH_PINS.contains(&gpio) {
                w.push(Violation::FlashPins(gpio).token());
            }
            if C3_USB_PINS.contains(&gpio) {
                w.push(Violation::UsbPins(gpio).token());
            }
            if C3_STRAPPING_HIGH.contains(&gpio) {
                w.push(Violation::StrappingHigh(gpio).token());
            }
            if gpio == 5 {
                w.push(Violation::Adc2Wifi(gpio).token());
            }
        }
        Soc::Esp32 => {
            const ADC2: [u16; 10] = [0, 2, 4, 12, 13, 14, 15, 25, 26, 27];
            if ESP32_FLASH_PINS.contains(&gpio) {
                w.push(Violation::FlashPins(gpio).token());
            }
            if ESP32_STRAPPING_HIGH.contains(&gpio) {
                w.push(Violation::StrappingHigh(gpio).token());
            }
            if ESP32_STRAPPING_LOW.contains(&gpio) {
                w.push(Violation::StrappingLow(gpio).token());
            }
            if ESP32_INPUT_ONLY.contains(&gpio) {
                w.push(Violation::InputOnlyPin(gpio).token());
            }
            if ADC2.contains(&gpio) {
                w.push(Violation::Adc2Wifi(gpio).token());
            }
            if is_console_pin(soc, gpio) {
                w.push(Violation::ConsolePins(gpio).token());
            }
        }
        Soc::Esp32S3 => {
            if S3_FLASH_PINS.contains(&gpio) {
                w.push(Violation::FlashPins(gpio).token());
            }
            if S3_PSRAM_PINS.contains(&gpio) {
                w.push(Violation::PsramPins(gpio).token());
            }
            if S3_STRAPPING_HIGH.contains(&gpio) {
                w.push(Violation::StrappingHigh(gpio).token());
            }
            if S3_INPUT_ONLY.contains(&gpio) {
                w.push(Violation::InputOnlyPin(gpio).token());
            }
            if (11..=20).contains(&gpio) {
                w.push(Violation::Adc2Wifi(gpio).token());
            }
            if gpio == 43 || gpio == 44 {
                w.push(format!("caps-uart-console:{gpio}"));
            }
        }
    }
    w
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

/// Validation d'un pin contre les chip-caps du SoC — **point unique** utilisé
/// à l'admission, avant chaque push de commande et au cast régulation
/// (brick0.md §2/§6/§8).
pub fn validate(
    soc: Soc,
    gpio: u16,
    mode: Mode,
    opts: &ModeOpts,
) -> Result<ValidatedPin, Violation> {
    match soc {
        Soc::Esp8266 => validate_esp8266(gpio, mode, opts),
        Soc::Esp32C3 => validate_esp32c3(gpio, mode, opts),
        Soc::Esp32 => validate_esp32(gpio, mode, opts),
        Soc::Esp32S3 => validate_esp32s3(gpio, mode, opts),
    }
}

/// ESP32-S3 — full GPIO matrix, ADC1 = GPIO1–10, no input-only pin except
/// GPIO46, strapping boot-HIGH on 0/3/45/46.
fn validate_esp32s3(gpio: u16, mode: Mode, opts: &ModeOpts) -> Result<ValidatedPin, Violation> {
    if !is_valid_gpio_esp32s3(gpio) {
        if S3_FLASH_PINS.contains(&gpio) {
            return Err(Violation::FlashPins(gpio));
        }
        if S3_PSRAM_PINS.contains(&gpio) {
            return Err(Violation::PsramPins(gpio));
        }
        if S3_USB_PINS.contains(&gpio) {
            return Err(Violation::UsbPins(gpio));
        }
        return Err(Violation::OutOfRange(gpio));
    }
    match mode {
        // ADC1 only (GPIO1–10) — ADC2 broken with WiFi active.
        Mode::AdcIn if !S3_ADC1_PINS.contains(&gpio) => return Err(Violation::Adc2Wifi(gpio)),
        _ => {}
    }
    let pullup = opts.pullup.unwrap_or(false);
    let safe_state = opts.safe_state.unwrap_or(SafeState::Low);
    if matches!(mode, Mode::DigitalOut | Mode::PwmOut) {
        if S3_INPUT_ONLY.contains(&gpio) {
            return Err(Violation::InputOnlyPin(gpio));
        }
        if safe_state == SafeState::Low && S3_STRAPPING_HIGH.contains(&gpio) {
            return Err(Violation::StrappingHigh(gpio));
        }
    }
    if S3_INPUT_ONLY.contains(&gpio) && pullup {
        return Err(Violation::NoPullUp(gpio));
    }
    Ok(ValidatedPin {
        gpio,
        mode,
        pullup,
        safe_state,
    })
}

fn validate_esp8266(gpio: u16, mode: Mode, opts: &ModeOpts) -> Result<ValidatedPin, Violation> {
    if !is_valid_gpio(gpio) {
        if FLASH_PINS.contains(&gpio) {
            return Err(Violation::FlashPins(gpio));
        }
        return Err(Violation::OutOfRange(gpio));
    }
    if is_console_pin(Soc::Esp8266, gpio) {
        return Err(Violation::ConsolePins(gpio));
    }
    match mode {
        Mode::AdcIn if gpio != A0_GPIO => return Err(Violation::AnalogOnlyOnA0(gpio)),
        m if m != Mode::AdcIn && gpio == A0_GPIO => return Err(Violation::AdcOnlyOnA0(gpio)),
        _ => {}
    }
    // GPIO16 : pas de timer waveform — digital only (interrupt/PWM exclus).
    if mode == Mode::PwmOut && gpio == NO_PULLUP {
        return Err(Violation::PwmUnsupportedPin(gpio));
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

fn validate_esp32c3(gpio: u16, mode: Mode, opts: &ModeOpts) -> Result<ValidatedPin, Violation> {
    if !is_valid_gpio_esp32c3(gpio) {
        if C3_FLASH_PINS.contains(&gpio) {
            return Err(Violation::FlashPins(gpio));
        }
        if C3_USB_PINS.contains(&gpio) {
            return Err(Violation::UsbPins(gpio));
        }
        return Err(Violation::OutOfRange(gpio));
    }
    match mode {
        // ADC1 only (GPIO0–4) — GPIO5 = ADC2 cassé avec le WiFi actif.
        Mode::AdcIn if !C3_ADC1_PINS.contains(&gpio) => return Err(Violation::Adc2Wifi(gpio)),
        _ => {}
    }
    let pullup = opts.pullup.unwrap_or(false);
    let safe_state = opts.safe_state.unwrap_or(SafeState::Low);
    if matches!(mode, Mode::DigitalOut | Mode::PwmOut)
        && safe_state == SafeState::Low
        && C3_STRAPPING_HIGH.contains(&gpio)
    {
        return Err(Violation::StrappingHigh(gpio));
    }
    Ok(ValidatedPin {
        gpio,
        mode,
        pullup,
        safe_state,
    })
}

/// Pins valides esp32 (classique) = GPIO0–39 sauf flash 6–11 et les GPIOs
/// inexistants (20, 24, 28–31).
pub fn is_valid_gpio_esp32(gpio: u16) -> bool {
    gpio <= 39 && !ESP32_FLASH_PINS.contains(&gpio) && !ESP32_ABSENT_PINS.contains(&gpio)
}

fn validate_esp32(gpio: u16, mode: Mode, opts: &ModeOpts) -> Result<ValidatedPin, Violation> {
    if !is_valid_gpio_esp32(gpio) {
        if ESP32_FLASH_PINS.contains(&gpio) {
            return Err(Violation::FlashPins(gpio));
        }
        return Err(Violation::OutOfRange(gpio));
    }
    if is_console_pin(Soc::Esp32, gpio) {
        return Err(Violation::ConsolePins(gpio));
    }
    match mode {
        // ADC1 only (GPIO32–39) — ADC2 cassé avec le WiFi actif.
        Mode::AdcIn if !ESP32_ADC1_PINS.contains(&gpio) => return Err(Violation::Adc2Wifi(gpio)),
        _ => {}
    }
    let pullup = opts.pullup.unwrap_or(false);
    let safe_state = opts.safe_state.unwrap_or(SafeState::Low);
    if matches!(mode, Mode::DigitalOut | Mode::PwmOut) {
        if safe_state == SafeState::Low && ESP32_STRAPPING_HIGH.contains(&gpio) {
            return Err(Violation::StrappingHigh(gpio));
        }
        if safe_state == SafeState::High && ESP32_STRAPPING_LOW.contains(&gpio) {
            return Err(Violation::StrappingLow(gpio));
        }
    }
    if ESP32_INPUT_ONLY.contains(&gpio) {
        if matches!(mode, Mode::DigitalOut | Mode::PwmOut) {
            return Err(Violation::InputOnlyPin(gpio));
        }
        if pullup {
            return Err(Violation::NoPullUp(gpio));
        }
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
        let s = Soc::Esp8266;
        // flash SPI 6-11 interdits
        let e = validate(s, 8, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::FlashPins(8)));
        assert!(e.reason().contains("flash SPI"));
        // hors plage
        let e = validate(s, 20, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::OutOfRange(20)));
        // analog only sur A0 (17) : digital refusé sur A0, analog refusé ailleurs
        let e = validate(s, 17, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::AdcOnlyOnA0(17)));
        let e = validate(s, 5, Mode::AdcIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::AnalogOnlyOnA0(5)));
        // GPIO15 : safe_state high interdit (strapping boot-LOW)
        let e = validate(s, 15, Mode::DigitalOut, &opts(None, Some(SafeState::High))).unwrap_err();
        assert!(matches!(e, Violation::StrappingLow(15)));
        // GPIO16 : pull-up inexistante
        let e = validate(s, 16, Mode::DigitalIn, &opts(Some(true), None)).unwrap_err();
        assert!(matches!(e, Violation::NoPullUp(16)));
        // GPIO16 : pas de timer waveform — pwm_out refusé (digital only)
        let e = validate(s, 16, Mode::PwmOut, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::PwmUnsupportedPin(16)));
        assert!(e.reason().contains("pwm_out"));
        // pwm_out valide sur un GPIO libre
        assert!(validate(s, 5, Mode::PwmOut, &opts(None, None)).is_ok());
    }

    #[test]
    fn regles_chip_caps_esp32c3() {
        let s = Soc::Esp32C3;
        // flash SPI interne 11-17 interdite
        let e = validate(s, 12, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::FlashPins(12)));
        // USB 18/19 réservés (port USB-JTAG/serial de flash/monitor)
        let e = validate(s, 19, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::UsbPins(19)));
        assert!(e.reason().contains("USB"));
        // hors plage (le C3 s'arrête à 21)
        let e = validate(s, 22, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::OutOfRange(22)));
        // ADC1 only : GPIO5 (ADC2) refusé en analog, GPIO0–4 admis
        let e = validate(s, 5, Mode::AdcIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::Adc2Wifi(5)));
        assert!(validate(s, 2, Mode::AdcIn, &opts(None, None)).is_ok());
        assert!(validate(s, 4, Mode::AdcIn, &opts(None, None)).is_ok());
        // strapping boot-HIGH (2/8/9) : safe_state low interdit en digital_out
        let e = validate(s, 9, Mode::DigitalOut, &opts(None, Some(SafeState::Low))).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(9)));
        let e = validate(s, 2, Mode::DigitalOut, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(2)));
        // ... mais HIGH est permis, et les autres modes aussi (pas de drive)
        assert!(validate(s, 9, Mode::DigitalOut, &opts(None, Some(SafeState::High))).is_ok());
        assert!(validate(s, 8, Mode::DigitalIn, &opts(Some(true), None)).is_ok());
        // pwm_out : mêmes règles strapping que digital_out
        let e = validate(s, 9, Mode::PwmOut, &opts(None, Some(SafeState::Low))).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(9)));
        assert!(validate(s, 1, Mode::PwmOut, &opts(None, None)).is_ok());
        // pins libres : GPIO1, 10, 20, 21
        assert!(validate(s, 1, Mode::DigitalOut, &opts(None, None)).is_ok());
        assert!(validate(s, 10, Mode::DigitalIn, &opts(None, None)).is_ok());
        assert!(validate(s, 20, Mode::DigitalIn, &opts(None, None)).is_ok());
        assert!(validate(s, 21, Mode::DigitalOut, &opts(None, Some(SafeState::High))).is_ok());
    }

    #[test]
    fn soc_depuis_board_soc() {
        assert_eq!(Soc::from_board_soc("esp8266"), Some(Soc::Esp8266));
        assert_eq!(Soc::from_board_soc("ESP8266"), Some(Soc::Esp8266));
        assert_eq!(Soc::from_board_soc("esp32-c3"), Some(Soc::Esp32C3));
        assert_eq!(Soc::from_board_soc("esp32c3"), Some(Soc::Esp32C3));
        assert_eq!(Soc::from_board_soc("esp32"), Some(Soc::Esp32));
        assert_eq!(Soc::from_board_soc("esp32-s3"), Some(Soc::Esp32S3));
        assert_eq!(Soc::from_board_soc("esp32s3"), Some(Soc::Esp32S3));
        assert_eq!(Soc::Esp32S3.name(), "esp32-s3");
        assert_eq!(Soc::from_board_soc("generic"), None);
    }

    #[test]
    fn regles_chip_caps_esp32s3() {
        let s = Soc::Esp32S3;
        // flash SPI interne 26-32 interdite
        let e = validate(s, 28, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::FlashPins(28)));
        // PSRAM octal 33-37 interdite (modules N8R8)
        let e = validate(s, 35, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::PsramPins(35)));
        assert!(e.reason().contains("PSRAM"));
        // USB 19/20 réservés (port USB-JTAG/serial de flash/monitor)
        let e = validate(s, 20, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::UsbPins(20)));
        // trous d'adressage 22-25 et hors plage
        let e = validate(s, 23, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::OutOfRange(23)));
        let e = validate(s, 49, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::OutOfRange(49)));
        // ADC1 only : GPIO1–10 admis, GPIO15 (ADC2) refusé
        assert!(validate(s, 5, Mode::AdcIn, &opts(None, None)).is_ok());
        let e = validate(s, 15, Mode::AdcIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::Adc2Wifi(15)));
        // GPIO46 input-only : sortie refusée, pull-up refusée
        let e = validate(s, 46, Mode::DigitalOut, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::InputOnlyPin(46)));
        let e = validate(s, 46, Mode::DigitalIn, &opts(Some(true), None)).unwrap_err();
        assert!(matches!(e, Violation::NoPullUp(46)));
        // strapping boot-HIGH (0/3/45/46) : safe_state low interdit en sortie
        let e = validate(s, 3, Mode::DigitalOut, &opts(None, Some(SafeState::Low))).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(3)));
        let e = validate(s, 45, Mode::PwmOut, &opts(None, Some(SafeState::Low))).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(45)));
        // ... mais HIGH est permis et les entrées aussi
        assert!(validate(s, 3, Mode::DigitalOut, &opts(None, Some(SafeState::High))).is_ok());
        assert!(validate(s, 0, Mode::DigitalIn, &opts(None, None)).is_ok());
        // pins libres : 10, 21, 38, 47, 48
        assert!(validate(s, 10, Mode::DigitalOut, &opts(None, None)).is_ok());
        assert!(validate(s, 21, Mode::DigitalIn, &opts(Some(true), None)).is_ok());
        assert!(validate(s, 38, Mode::DigitalOut, &opts(None, None)).is_ok());
        assert!(validate(s, 48, Mode::PwmOut, &opts(None, None)).is_ok());
        // modes affichés : GPIO46 jamais en sortie, GPIO8 tout sauf... adc1
        let m = available_modes(s, 46);
        assert_eq!(m, vec![Mode::DigitalIn]);
        let m = available_modes(s, 8);
        assert!(m.contains(&Mode::AdcIn) && m.contains(&Mode::DigitalOut));
    }

    #[test]
    fn regles_chip_caps_esp32() {
        let s = Soc::Esp32;
        // flash SPI 6-11 interdite
        let e = validate(s, 7, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::FlashPins(7)));
        // GPIO inexistants (adressage troué)
        let e = validate(s, 20, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::OutOfRange(20)));
        let e = validate(s, 31, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::OutOfRange(31)));
        let e = validate(s, 40, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::OutOfRange(40)));
        // ADC1 only (32–39) : GPIO25 (ADC2) refusé, GPIO33 admis
        let e = validate(s, 25, Mode::AdcIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::Adc2Wifi(25)));
        assert!(validate(s, 33, Mode::AdcIn, &opts(None, None)).is_ok());
        // input-only (34–39) : sortie refusée, pull-up refusée
        let e = validate(s, 35, Mode::DigitalOut, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::InputOnlyPin(35)));
        assert!(e.reason().contains("input-only"));
        let e = validate(s, 34, Mode::DigitalIn, &opts(Some(true), None)).unwrap_err();
        assert!(matches!(e, Violation::NoPullUp(34)));
        // ... mais digital_in sans pull-up et ADC1 passent sur input-only
        assert!(validate(s, 36, Mode::DigitalIn, &opts(None, None)).is_ok());
        assert!(validate(s, 39, Mode::AdcIn, &opts(None, None)).is_ok());
        // strapping boot-HIGH (0, 15) : safe_state low interdit en digital_out
        let e = validate(s, 0, Mode::DigitalOut, &opts(None, Some(SafeState::Low))).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(0)));
        let e = validate(s, 15, Mode::DigitalOut, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(15)));
        // strapping boot-LOW (2, 12) : safe_state high interdit en digital_out
        let e = validate(s, 12, Mode::DigitalOut, &opts(None, Some(SafeState::High))).unwrap_err();
        assert!(matches!(e, Violation::StrappingLow(12)));
        let e = validate(s, 2, Mode::DigitalOut, &opts(None, Some(SafeState::High))).unwrap_err();
        assert!(matches!(e, Violation::StrappingLow(2)));
        // ... modes autorisés sur les strapping : digital_in, out/High sur 0
        assert!(validate(s, 12, Mode::DigitalIn, &opts(None, None)).is_ok());
        assert!(validate(s, 0, Mode::DigitalOut, &opts(None, Some(SafeState::High))).is_ok());
        // pwm_out : input-only refusé, strapping et pin libre comme digital_out
        let e = validate(s, 35, Mode::PwmOut, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::InputOnlyPin(35)));
        let e = validate(s, 0, Mode::PwmOut, &opts(None, Some(SafeState::Low))).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(0)));
        assert!(validate(s, 4, Mode::PwmOut, &opts(None, None)).is_ok());
        // pins libres : 4, 13, 16, 27, 32
        assert!(validate(s, 4, Mode::DigitalOut, &opts(None, None)).is_ok());
        assert!(validate(s, 13, Mode::DigitalIn, &opts(Some(true), None)).is_ok());
        assert!(validate(s, 16, Mode::DigitalOut, &opts(None, Some(SafeState::High))).is_ok());
        assert!(validate(s, 27, Mode::DigitalIn, &opts(None, None)).is_ok());
        assert!(validate(s, 32, Mode::DigitalOut, &opts(None, None)).is_ok());
    }

    #[test]
    fn modes_disponibles_esp8266() {
        let s = Soc::Esp8266;
        // GPIO5 : in/out/pwm, pas d'analog
        assert_eq!(
            available_modes(s, 5),
            vec![Mode::DigitalIn, Mode::DigitalOut, Mode::PwmOut]
        );
        // GPIO16 : pas de timer waveform
        assert_eq!(
            available_modes(s, 16),
            vec![Mode::DigitalIn, Mode::DigitalOut]
        );
        // A0 : analogique seulement
        assert_eq!(available_modes(s, A0_GPIO), vec![Mode::AdcIn]);
        // flash : rien
        assert!(available_modes(s, 8).is_empty());
    }

    #[test]
    fn modes_disponibles_esp32c3() {
        let s = Soc::Esp32C3;
        // GPIO2 strapping-high : sorties STRUCTURELLEMENT offertes (légales
        // avec safe_state: high) même si validate(default) les refuserait
        let m = available_modes(s, 2);
        assert!(m.contains(&Mode::DigitalOut) && m.contains(&Mode::PwmOut));
        assert!(m.contains(&Mode::AdcIn), "GPIO2 = ADC1");
        // GPIO5 ADC2 : pas d'analog, mais in/out/pwm ok
        assert_eq!(
            available_modes(s, 5),
            vec![Mode::DigitalIn, Mode::DigitalOut, Mode::PwmOut]
        );
        // USB 18/19 : rien
        assert!(available_modes(s, 19).is_empty());
    }

    #[test]
    fn modes_disponibles_esp32() {
        let s = Soc::Esp32;
        // input-only 34-39 : in + analog, jamais de sortie
        assert_eq!(available_modes(s, 34), vec![Mode::DigitalIn, Mode::AdcIn]);
        assert_eq!(available_modes(s, 39), vec![Mode::DigitalIn, Mode::AdcIn]);
        // 25/26 ADC2+DAC : pas d'analog_in (ADC2 cassé WiFi), sorties ok
        assert_eq!(
            available_modes(s, 25),
            vec![Mode::DigitalIn, Mode::DigitalOut, Mode::PwmOut]
        );
        // 21/22 libres : tout sauf analog
        assert_eq!(
            available_modes(s, 21),
            vec![Mode::DigitalIn, Mode::DigitalOut, Mode::PwmOut]
        );
        // flash : rien
        assert!(available_modes(s, 9).is_empty());
    }

    #[test]
    fn flags_et_fns_affichage() {
        assert_eq!(pin_flags(Soc::Esp8266, A0_GPIO), &[PinFlag::InputOnly]);
        assert_eq!(pin_flags(Soc::Esp8266, 15), &[PinFlag::Strapping]);
        assert_eq!(pin_flags(Soc::Esp8266, 1), &[PinFlag::Reserved]);
        assert_eq!(pin_flags(Soc::Esp32C3, 18), &[PinFlag::Reserved]);
        assert_eq!(pin_flags(Soc::Esp32, 36), &[PinFlag::InputOnly]);
        assert!(pin_flags(Soc::Esp32, 23).is_empty());
        assert_eq!(pin_fns(Soc::Esp32C3, 5), &[PinFn::Adc2]);
        assert_eq!(pin_fns(Soc::Esp8266, A0_GPIO), &[PinFn::Adc1]);
        assert_eq!(
            pin_fns(Soc::Esp32, 2),
            &[PinFn::Adc2, PinFn::Touch, PinFn::Led]
        );
        // sérialisation snake_case pour le fil
        assert_eq!(
            serde_json::to_value(PinFn::I2cSda).unwrap(),
            serde_json::json!("i2c_sda")
        );
        assert_eq!(
            serde_json::to_value(PinFlag::InputOnly).unwrap(),
            serde_json::json!("input_only")
        );
    }

    /// UART0 console TX/RX is never a capability where the generic
    /// firmware keeps Serial on it (ESP8266, ESP32); C3/S3 log over USB-CDC.
    #[test]
    fn console_pins_rejected_where_serial_uses_uart0() {
        let opts = ModeOpts::default();
        for soc in [Soc::Esp8266, Soc::Esp32] {
            for gpio in [1, 3] {
                let e = validate(soc, gpio, Mode::DigitalIn, &opts).unwrap_err();
                assert_eq!(e, Violation::ConsolePins(gpio));
                assert!(available_modes(soc, gpio).is_empty(), "{soc:?} {gpio}");
            }
        }
        assert!(validate(Soc::Esp32C3, 21, Mode::DigitalIn, &opts).is_ok());
        assert!(validate(Soc::Esp32S3, 43, Mode::DigitalIn, &opts).is_ok());
    }

    #[test]
    fn avertissements_affichage() {
        let w = pin_warnings(Soc::Esp8266, 15);
        assert!(
            w.len() == 1 && w[0].starts_with("caps-strapping-low:"),
            "{w:?}"
        );
        let w = pin_warnings(Soc::Esp8266, 1);
        assert!(w[0].starts_with("caps-console-pins:"), "{w:?}");
        let w = pin_warnings(Soc::Esp32C3, 5);
        assert!(w.len() == 1 && w[0].starts_with("caps-adc2-wifi:"), "{w:?}");
        let w = pin_warnings(Soc::Esp32, 12);
        assert!(w[0].starts_with("caps-strapping-low:"), "{w:?}");
        let w = pin_warnings(Soc::Esp32, 6);
        assert!(w[0].starts_with("caps-flash-pins:"), "{w:?}");
        // A0 : rien à signaler
        assert!(pin_warnings(Soc::Esp8266, A0_GPIO).is_empty());
        // pin libre : rien
        assert!(pin_warnings(Soc::Esp8266, 5).is_empty());
    }
}
