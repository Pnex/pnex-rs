//! Chip-caps per SoC — the **silicon** constraint table (Brick 0 §1/§2).
//!
//! "Chip" level of the 3-layer model: chip-caps in **code** (this file),
//! board overlay in **data** (`mcu_boards.details`), capability instance in
//! **PG** (`device_capability_instances`).
//!
//! `validate` is the **single point** of pin validation: used by the
//! backend at admission (`Announce` → provisioning), before every command
//! push (POST commands) and when casting control cards (regulator.rs)
//! — an illegal op is rejected 400 with a reason, never pushed to the device
//! (brick0.md §6/§8).
//!
//! ESP8266 (P0):
//! - GPIO6–11 = SPI flash → **forbidden**;
//! - GPIO0/2/15 = strapping pins (0/2 = HIGH expected at boot, 15 = LOW);
//! - GPIO16: no interrupt/PWM, **pulldown only** (no pull-up);
//! - A0 = single 10-bit ADC channel (0–1023), no physical GPIO — agreed wire
//!   identifier `A0_GPIO`.
//!
//! ESP32-C3 (P1, Seeed XIAO):
//! - GPIO11–17 = internal SPI flash → **forbidden**;
//! - GPIO18/19 = USB D-/D+ (USB-JTAG/serial = the flash/monitor port) →
//!   **reserved**;
//! - GPIO2/8/9 = boot-HIGH strapping (external XIAO pull-ups) — `safe_state:
//!   low` forbidden in digital_out (risk of booting in download mode);
//! - analog_in limited to GPIO0–4 (ADC1) — GPIO5 = ADC2, unusable with
//!   WiFi active.
use serde::{Deserialize, Serialize};

use crate::proto::{Mode, ModeOpts, SafeState};

/// Wire identifier of the ADC channel ("A0") — **no physical GPIO 17** on
/// the ESP8266: it is the conventional address of the ADC, used by
/// the NodeMCU overlay and the firmware (`analogRead(A0)` read).
pub const A0_GPIO: u16 = 17;

/// Strapping GPIO "must be LOW at boot" (GPIO15 = D8).
pub const STRAPPING_LOW: u16 = 15;
/// Strapping GPIOs "must be HIGH at boot" (GPIO0 = D3, GPIO2 = D4).
pub const STRAPPING_HIGH: [u16; 2] = [0, 2];
/// SPI flash GPIOs — forbidden as capability (GPIO6–11).
pub const FLASH_PINS: [u16; 6] = [6, 7, 8, 9, 10, 11];
/// GPIO without internal pull-up (GPIO16 = D0 — pulldown only).
pub const NO_PULLUP: u16 = 16;

// ─────────────────────── ESP32-C3 ───────────────────────

/// Internal SPI flash (GPIO11–17) — forbidden as capability (esp32-c3).
pub const C3_FLASH_PINS: [u16; 7] = [11, 12, 13, 14, 15, 16, 17];
/// USB D-/D+ (GPIO18/19) — reserved: they are the USB flash/monitor port
/// (USB-JTAG/serial). A capability on them would kill access to the device.
pub const C3_USB_PINS: [u16; 2] = [18, 19];
/// Boot-HIGH strapping (GPIO2/8/9) — external XIAO pull-ups; forcing LOW at
/// boot = risk of download mode.
pub const C3_STRAPPING_HIGH: [u16; 3] = [2, 8, 9];
/// ADC1 (GPIO0–4) — the only ADC channels usable with WiFi active
/// (GPIO5 = ADC2, broken with WiFi active).
pub const C3_ADC1_PINS: [u16; 5] = [0, 1, 2, 3, 4];

// ─────────────────────── ESP32-C6 ───────────────────────
// P3 (Waveshare ESP32-C6-Zero, ESP32-C6FH8: in-package flash). Arduino core
// 3.x only (pioarduino platform).

/// In-package / SPI flash (GPIO24–30) — forbidden for capabilities.
pub const C6_FLASH_PINS: [u16; 7] = [24, 25, 26, 27, 28, 29, 30];
/// USB D-/D+ (GPIO12/13) — reserved: USB-JTAG/serial flash/monitor port.
pub const C6_USB_PINS: [u16; 2] = [12, 13];
/// Strapping boot-HIGH (GPIO8/9: GPIO9 low at boot = download mode, GPIO8
/// must then be high): `safe_state: low` forbidden on outputs.
pub const C6_STRAPPING_HIGH: [u16; 2] = [8, 9];
/// ADC1 (GPIO0–6) — the C6 has no ADC2.
pub const C6_ADC1_PINS: [u16; 7] = [0, 1, 2, 3, 4, 5, 6];

// ─────────────────────── ESP32 (classic) ───────────────────────
// Custom firmware on a classic ESP32 (pio board `esp32dev`) — same rule
// grid as the C3: protected strapping, ADC1 only, input-only without pull-up.

/// SPI flash (GPIO6–11) — forbidden as capability (classic esp32).
pub const ESP32_FLASH_PINS: [u16; 6] = [6, 7, 8, 9, 10, 11];
/// GPIOs that do not exist on the classic ESP32 (gapped addressing).
const ESP32_ABSENT_PINS: [u16; 6] = [20, 24, 28, 29, 30, 31];
/// Boot-HIGH strapping (GPIO0, GPIO15 — must be HIGH/floating at
/// boot): `safe_state: low` forbidden in digital_out.
pub const ESP32_STRAPPING_HIGH: [u16; 2] = [0, 15];
/// Boot-LOW strapping (GPIO12 = MTDI — flash voltage; GPIO2 = required low
/// in download mode): `safe_state: high` forbidden in digital_out.
pub const ESP32_STRAPPING_LOW: [u16; 2] = [2, 12];
/// Input-only (GPIO34–39): no output, no internal pull-up.
pub const ESP32_INPUT_ONLY: [u16; 6] = [34, 35, 36, 37, 38, 39];
/// ADC1 (GPIO32–39) — the only channels usable with WiFi active (ADC2
/// broken with WiFi). 37/38 taken by the PSRAM on WROVER, free on
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

/// SoCs known to the chip-caps — derived from `mcu_boards.soc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Soc {
    Esp8266,
    Esp32C3,
    Esp32C6,
    Esp32,
    Esp32S3,
}

impl Soc {
    /// `mcu_boards.soc` / chip announce convention ("esp8266",
    /// "esp32-c3", "esp32", "esp32-s3") → SoC. Unknown → `None`: the
    /// call-sites fail closed (never a silent validation against another
    /// SoC's rules).
    pub fn from_board_soc(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "esp8266" => Some(Soc::Esp8266),
            "esp32-c3" | "esp32c3" => Some(Soc::Esp32C3),
            "esp32-c6" | "esp32c6" => Some(Soc::Esp32C6),
            "esp32" => Some(Soc::Esp32),
            "esp32-s3" | "esp32s3" => Some(Soc::Esp32S3),
            _ => None,
        }
    }

    /// Canonical wire name ("esp8266", "esp32-c3", "esp32") — chip
    /// announced by the PneX lib and value persisted in
    /// `device_registries.soc` (SoC observed at the first announce).
    pub fn name(self) -> &'static str {
        match self {
            Soc::Esp8266 => "esp8266",
            Soc::Esp32C3 => "esp32-c3",
            Soc::Esp32C6 => "esp32-c6",
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

/// Valid P0 (esp8266) pins = every GPIO 0–16 except the flash, plus the ADC channel.
pub fn is_valid_gpio(gpio: u16) -> bool {
    gpio == A0_GPIO || (gpio <= 16 && !FLASH_PINS.contains(&gpio))
}

/// Valid esp32-c3 pins = GPIO0–10, 20, 21 (the 11–17 flash and the 18/19 USB
/// are reserved).
pub fn is_valid_gpio_esp32c3(gpio: u16) -> bool {
    gpio <= 21 && !C3_FLASH_PINS.contains(&gpio) && !C3_USB_PINS.contains(&gpio)
}

/// Valid esp32-c6 pins = GPIO0–23 except USB 12/13 (flash 24–30 forbidden).
pub fn is_valid_gpio_esp32c6(gpio: u16) -> bool {
    gpio <= 23 && !C6_USB_PINS.contains(&gpio)
}

/// Chip-caps violation — `reason()` gives the wire/UI message (relayed
/// as is by the front: "relayed errors" convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Violation {
    /// GPIO that does not exist or is outside ESP8266 addressing.
    OutOfRange(u16),
    /// GPIO6–11 (flash SPI).
    FlashPins(u16),
    /// Octal PSRAM pins (esp32-s3: GPIO33–37).
    PsramPins(u16),
    /// USB D-/D+ (esp32-c3: 18/19) — reserved for the USB-JTAG/serial port.
    UsbPins(u16),
    /// UART0 console pins kept by the generic firmware's `Serial` (see
    /// [`is_console_pin`]) — never user capabilities.
    ConsolePins(u16),
    /// analog_in outside ADC1 (esp32-c3: GPIO5 = ADC2, broken with WiFi).
    Adc2Wifi(u16),
    /// Analog mode outside A0.
    AnalogOnlyOnA0(u16),
    /// Digital mode on A0 (the ADC has no digital).
    AdcOnlyOnA0(u16),
    /// `safe_state: high` on GPIO15 — would force a boot in SD-card mode.
    StrappingLow(u16),
    /// `safe_state: low` on boot-HIGH strapping (esp32-c3: GPIO2/8/9) —
    /// risk of booting in download mode.
    StrappingHigh(u16),
    /// Output requested on an input-only pin (esp32: GPIO34–39).
    InputOnlyPin(u16),
    /// Pull-up requested on GPIO16 (does not physically exist).
    NoPullUp(u16),
    /// pwm_out requested on a GPIO without a waveform timer (esp8266: GPIO16).
    PwmUnsupportedPin(u16),
}

impl Violation {
    /// Wire/UI message (relayed as is — "relayed errors" convention).
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
            0..=4 => &[PinFn::Adc1],
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
        // PneX screen conventions (the C6 routes any peripheral to any pin:
        // display hints only).
        Soc::Esp32C6 => match gpio {
            0..=6 => &[PinFn::Adc1],
            8 => &[PinFn::Led],
            16 => &[PinFn::UartTx],
            17 => &[PinFn::UartRx],
            18 => &[PinFn::SpiCs],
            19 => &[PinFn::SpiClk],
            20 => &[PinFn::SpiMosi],
            21 => &[PinFn::I2cSda],
            22 => &[PinFn::I2cScl],
            _ => NONE,
        },
        Soc::Esp32 => match gpio {
            32 | 33 => &[PinFn::Adc1, PinFn::Touch],
            34..=39 => &[PinFn::Adc1],
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
            1..=7 => &[PinFn::Adc1],
            8 => &[PinFn::Adc1, PinFn::I2cSda],
            9 => &[PinFn::Adc1, PinFn::I2cScl],
            10 => &[PinFn::Adc1, PinFn::SpiCs],
            11 => &[PinFn::Adc2, PinFn::SpiMosi],
            12 => &[PinFn::Adc2, PinFn::SpiClk],
            13 => &[PinFn::Adc2, PinFn::SpiMiso],
            14..=20 => &[PinFn::Adc2],
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
        Soc::Esp32C6 => match gpio {
            4 | 5 | 8 | 9 | 15 => &[PinFlag::Strapping],
            12 | 13 => &[PinFlag::Reserved],
            _ => NONE,
        },
        Soc::Esp32 => match gpio {
            34..=39 => &[PinFlag::InputOnly],
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
        Soc::Esp32C6 => is_valid_gpio_esp32c6(gpio),
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
                return w; // ADC convention: nothing to report
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
        Soc::Esp32C6 => {
            if C6_FLASH_PINS.contains(&gpio) {
                w.push(Violation::FlashPins(gpio).token());
            }
            if C6_USB_PINS.contains(&gpio) {
                w.push(Violation::UsbPins(gpio).token());
            }
            if C6_STRAPPING_HIGH.contains(&gpio) {
                w.push(Violation::StrappingHigh(gpio).token());
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

/// Validated pin — serialized as is in `constraints_snapshot` (jsonb):
/// what was validated at admission, replayable by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatedPin {
    pub gpio: u16,
    pub mode: Mode,
    /// Effective pull-up (default false).
    pub pullup: bool,
    /// Effective safe-state (default Low).
    pub safe_state: SafeState,
}

/// Validation of a pin against the SoC chip-caps — **single point** used
/// at admission, before every command push and at control cast
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
        Soc::Esp32C6 => validate_esp32c6(gpio, mode, opts),
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
    // GPIO16: no waveform timer — digital only (interrupt/PWM excluded).
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
        // ADC1 only (GPIO0–4) — GPIO5 = ADC2 broken with WiFi active.
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

/// ESP32-C6 — GPIO matrix, ADC1 = GPIO0–6 (no ADC2), strapping boot-HIGH
/// on 8/9, no input-only pin.
fn validate_esp32c6(gpio: u16, mode: Mode, opts: &ModeOpts) -> Result<ValidatedPin, Violation> {
    if !is_valid_gpio_esp32c6(gpio) {
        if C6_FLASH_PINS.contains(&gpio) {
            return Err(Violation::FlashPins(gpio));
        }
        if C6_USB_PINS.contains(&gpio) {
            return Err(Violation::UsbPins(gpio));
        }
        return Err(Violation::OutOfRange(gpio));
    }
    if mode == Mode::AdcIn && !C6_ADC1_PINS.contains(&gpio) {
        return Err(Violation::Adc2Wifi(gpio));
    }
    let pullup = opts.pullup.unwrap_or(false);
    let safe_state = opts.safe_state.unwrap_or(SafeState::Low);
    if matches!(mode, Mode::DigitalOut | Mode::PwmOut)
        && safe_state == SafeState::Low
        && C6_STRAPPING_HIGH.contains(&gpio)
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

/// Valid esp32 (classic) pins = GPIO0–39 except flash 6–11 and the GPIOs
/// that do not exist (20, 24, 28–31).
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
        // ADC1 only (GPIO32–39) — ADC2 broken with WiFi active.
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
    fn chip_caps_rules_esp8266() {
        let s = Soc::Esp8266;
        // SPI flash 6-11 forbidden
        let e = validate(s, 8, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::FlashPins(8)));
        assert!(e.reason().contains("flash SPI"));
        // out of range
        let e = validate(s, 20, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::OutOfRange(20)));
        // analog only on A0 (17): digital refused on A0, analog refused elsewhere
        let e = validate(s, 17, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::AdcOnlyOnA0(17)));
        let e = validate(s, 5, Mode::AdcIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::AnalogOnlyOnA0(5)));
        // GPIO15: safe_state high forbidden (boot-LOW strapping)
        let e = validate(s, 15, Mode::DigitalOut, &opts(None, Some(SafeState::High))).unwrap_err();
        assert!(matches!(e, Violation::StrappingLow(15)));
        // GPIO16: no pull-up
        let e = validate(s, 16, Mode::DigitalIn, &opts(Some(true), None)).unwrap_err();
        assert!(matches!(e, Violation::NoPullUp(16)));
        // GPIO16: no waveform timer — pwm_out refused (digital only)
        let e = validate(s, 16, Mode::PwmOut, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::PwmUnsupportedPin(16)));
        assert!(e.reason().contains("pwm_out"));
        // pwm_out valid on a free GPIO
        assert!(validate(s, 5, Mode::PwmOut, &opts(None, None)).is_ok());
    }

    #[test]
    fn chip_caps_rules_esp32c3() {
        let s = Soc::Esp32C3;
        // internal SPI flash 11-17 forbidden
        let e = validate(s, 12, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::FlashPins(12)));
        // USB 18/19 reserved (USB-JTAG/serial flash/monitor port)
        let e = validate(s, 19, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::UsbPins(19)));
        assert!(e.reason().contains("USB"));
        // out of range (the C3 stops at 21)
        let e = validate(s, 22, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::OutOfRange(22)));
        // ADC1 only: GPIO5 (ADC2) refused in analog, GPIO0–4 admitted
        let e = validate(s, 5, Mode::AdcIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::Adc2Wifi(5)));
        assert!(validate(s, 2, Mode::AdcIn, &opts(None, None)).is_ok());
        assert!(validate(s, 4, Mode::AdcIn, &opts(None, None)).is_ok());
        // boot-HIGH strapping (2/8/9): safe_state low forbidden in digital_out
        let e = validate(s, 9, Mode::DigitalOut, &opts(None, Some(SafeState::Low))).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(9)));
        let e = validate(s, 2, Mode::DigitalOut, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(2)));
        // ... but HIGH is allowed, and so are the other modes (no drive)
        assert!(validate(s, 9, Mode::DigitalOut, &opts(None, Some(SafeState::High))).is_ok());
        assert!(validate(s, 8, Mode::DigitalIn, &opts(Some(true), None)).is_ok());
        // pwm_out: same strapping rules as digital_out
        let e = validate(s, 9, Mode::PwmOut, &opts(None, Some(SafeState::Low))).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(9)));
        assert!(validate(s, 1, Mode::PwmOut, &opts(None, None)).is_ok());
        // free pins: GPIO1, 10, 20, 21
        assert!(validate(s, 1, Mode::DigitalOut, &opts(None, None)).is_ok());
        assert!(validate(s, 10, Mode::DigitalIn, &opts(None, None)).is_ok());
        assert!(validate(s, 20, Mode::DigitalIn, &opts(None, None)).is_ok());
        assert!(validate(s, 21, Mode::DigitalOut, &opts(None, Some(SafeState::High))).is_ok());
    }

    #[test]
    fn soc_from_board_soc() {
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
    fn chip_caps_rules_esp32s3() {
        let s = Soc::Esp32S3;
        // internal SPI flash 26-32 forbidden
        let e = validate(s, 28, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::FlashPins(28)));
        // octal PSRAM 33-37 forbidden (N8R8 modules)
        let e = validate(s, 35, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::PsramPins(35)));
        assert!(e.reason().contains("PSRAM"));
        // USB 19/20 reserved (USB-JTAG/serial flash/monitor port)
        let e = validate(s, 20, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::UsbPins(20)));
        // addressing gaps 22-25 and out of range
        let e = validate(s, 23, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::OutOfRange(23)));
        let e = validate(s, 49, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::OutOfRange(49)));
        // ADC1 only: GPIO1–10 admitted, GPIO15 (ADC2) refused
        assert!(validate(s, 5, Mode::AdcIn, &opts(None, None)).is_ok());
        let e = validate(s, 15, Mode::AdcIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::Adc2Wifi(15)));
        // GPIO46 input-only: output refused, pull-up refused
        let e = validate(s, 46, Mode::DigitalOut, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::InputOnlyPin(46)));
        let e = validate(s, 46, Mode::DigitalIn, &opts(Some(true), None)).unwrap_err();
        assert!(matches!(e, Violation::NoPullUp(46)));
        // boot-HIGH strapping (0/3/45/46): safe_state low forbidden as output
        let e = validate(s, 3, Mode::DigitalOut, &opts(None, Some(SafeState::Low))).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(3)));
        let e = validate(s, 45, Mode::PwmOut, &opts(None, Some(SafeState::Low))).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(45)));
        // ... but HIGH is allowed, and so are inputs
        assert!(validate(s, 3, Mode::DigitalOut, &opts(None, Some(SafeState::High))).is_ok());
        assert!(validate(s, 0, Mode::DigitalIn, &opts(None, None)).is_ok());
        // free pins: 10, 21, 38, 47, 48
        assert!(validate(s, 10, Mode::DigitalOut, &opts(None, None)).is_ok());
        assert!(validate(s, 21, Mode::DigitalIn, &opts(Some(true), None)).is_ok());
        assert!(validate(s, 38, Mode::DigitalOut, &opts(None, None)).is_ok());
        assert!(validate(s, 48, Mode::PwmOut, &opts(None, None)).is_ok());
        // displayed modes: GPIO46 never as output, GPIO8 everything except... adc1
        let m = available_modes(s, 46);
        assert_eq!(m, vec![Mode::DigitalIn]);
        let m = available_modes(s, 8);
        assert!(m.contains(&Mode::AdcIn) && m.contains(&Mode::DigitalOut));
    }

    #[test]
    fn chip_caps_rules_esp32() {
        let s = Soc::Esp32;
        // SPI flash 6-11 forbidden
        let e = validate(s, 7, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::FlashPins(7)));
        // GPIOs that do not exist (gapped addressing)
        let e = validate(s, 20, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::OutOfRange(20)));
        let e = validate(s, 31, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::OutOfRange(31)));
        let e = validate(s, 40, Mode::DigitalIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::OutOfRange(40)));
        // ADC1 only (32–39): GPIO25 (ADC2) refused, GPIO33 admitted
        let e = validate(s, 25, Mode::AdcIn, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::Adc2Wifi(25)));
        assert!(validate(s, 33, Mode::AdcIn, &opts(None, None)).is_ok());
        // input-only (34–39): output refused, pull-up refused
        let e = validate(s, 35, Mode::DigitalOut, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::InputOnlyPin(35)));
        assert!(e.reason().contains("input-only"));
        let e = validate(s, 34, Mode::DigitalIn, &opts(Some(true), None)).unwrap_err();
        assert!(matches!(e, Violation::NoPullUp(34)));
        // ... but digital_in without pull-up and ADC1 pass on input-only
        assert!(validate(s, 36, Mode::DigitalIn, &opts(None, None)).is_ok());
        assert!(validate(s, 39, Mode::AdcIn, &opts(None, None)).is_ok());
        // boot-HIGH strapping (0, 15): safe_state low forbidden in digital_out
        let e = validate(s, 0, Mode::DigitalOut, &opts(None, Some(SafeState::Low))).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(0)));
        let e = validate(s, 15, Mode::DigitalOut, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(15)));
        // boot-LOW strapping (2, 12): safe_state high forbidden in digital_out
        let e = validate(s, 12, Mode::DigitalOut, &opts(None, Some(SafeState::High))).unwrap_err();
        assert!(matches!(e, Violation::StrappingLow(12)));
        let e = validate(s, 2, Mode::DigitalOut, &opts(None, Some(SafeState::High))).unwrap_err();
        assert!(matches!(e, Violation::StrappingLow(2)));
        // ... modes allowed on strapping pins: digital_in, out/High on 0
        assert!(validate(s, 12, Mode::DigitalIn, &opts(None, None)).is_ok());
        assert!(validate(s, 0, Mode::DigitalOut, &opts(None, Some(SafeState::High))).is_ok());
        // pwm_out: input-only refused, strapping and free pin like digital_out
        let e = validate(s, 35, Mode::PwmOut, &opts(None, None)).unwrap_err();
        assert!(matches!(e, Violation::InputOnlyPin(35)));
        let e = validate(s, 0, Mode::PwmOut, &opts(None, Some(SafeState::Low))).unwrap_err();
        assert!(matches!(e, Violation::StrappingHigh(0)));
        assert!(validate(s, 4, Mode::PwmOut, &opts(None, None)).is_ok());
        // free pins: 4, 13, 16, 27, 32
        assert!(validate(s, 4, Mode::DigitalOut, &opts(None, None)).is_ok());
        assert!(validate(s, 13, Mode::DigitalIn, &opts(Some(true), None)).is_ok());
        assert!(validate(s, 16, Mode::DigitalOut, &opts(None, Some(SafeState::High))).is_ok());
        assert!(validate(s, 27, Mode::DigitalIn, &opts(None, None)).is_ok());
        assert!(validate(s, 32, Mode::DigitalOut, &opts(None, None)).is_ok());
    }

    #[test]
    fn available_modes_esp8266() {
        let s = Soc::Esp8266;
        // GPIO5: in/out/pwm, no analog
        assert_eq!(
            available_modes(s, 5),
            vec![Mode::DigitalIn, Mode::DigitalOut, Mode::PwmOut]
        );
        // GPIO16: no waveform timer
        assert_eq!(
            available_modes(s, 16),
            vec![Mode::DigitalIn, Mode::DigitalOut]
        );
        // A0: analog only
        assert_eq!(available_modes(s, A0_GPIO), vec![Mode::AdcIn]);
        // flash: nothing
        assert!(available_modes(s, 8).is_empty());
    }

    #[test]
    fn available_modes_esp32c3() {
        let s = Soc::Esp32C3;
        // GPIO2 strapping-high: outputs STRUCTURALLY offered (legal
        // with safe_state: high) even if validate(default) would refuse them
        let m = available_modes(s, 2);
        assert!(m.contains(&Mode::DigitalOut) && m.contains(&Mode::PwmOut));
        assert!(m.contains(&Mode::AdcIn), "GPIO2 = ADC1");
        // GPIO5 ADC2: no analog, but in/out/pwm ok
        assert_eq!(
            available_modes(s, 5),
            vec![Mode::DigitalIn, Mode::DigitalOut, Mode::PwmOut]
        );
        // USB 18/19: nothing
        assert!(available_modes(s, 19).is_empty());
    }

    #[test]
    fn available_modes_esp32() {
        let s = Soc::Esp32;
        // input-only 34-39: in + analog, never an output
        assert_eq!(available_modes(s, 34), vec![Mode::DigitalIn, Mode::AdcIn]);
        assert_eq!(available_modes(s, 39), vec![Mode::DigitalIn, Mode::AdcIn]);
        // 25/26 ADC2+DAC: no analog_in (ADC2 broken with WiFi), outputs ok
        assert_eq!(
            available_modes(s, 25),
            vec![Mode::DigitalIn, Mode::DigitalOut, Mode::PwmOut]
        );
        // 21/22 free: everything except analog
        assert_eq!(
            available_modes(s, 21),
            vec![Mode::DigitalIn, Mode::DigitalOut, Mode::PwmOut]
        );
        // flash: nothing
        assert!(available_modes(s, 9).is_empty());
    }

    #[test]
    fn display_flags_and_fns() {
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
        // snake_case serialization for the wire
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
    fn display_warnings() {
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
        // A0: nothing to report
        assert!(pin_warnings(Soc::Esp8266, A0_GPIO).is_empty());
        // free pin: nothing
        assert!(pin_warnings(Soc::Esp8266, 5).is_empty());
    }
}
