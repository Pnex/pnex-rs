//! Board overlays — the board level of the 3-layer model (Brick 0 §1).
//!
//! An overlay describes a board's wiring (labels D0…D8/A0 → GPIO). It lives
//! in `mcu_boards.details` (jsonb, never in a `.h` — PRD §2.3) and is
//! deserialized server-side to derive the pin map at admission. The source
//! of the stored profiles is the typed registry `crate::catalog` (D121).

use serde::{Deserialize, Serialize};

use crate::caps;
use crate::proto::{Mode, ModeOpts, SafeState};

/// Carte de pins d'une board — contenu de `mcu_boards.details`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardOverlay {
    /// Identifiant de board (« nodemcu », « d1_mini »…).
    pub board: String,
    /// Pins exposés à l'utilisateur.
    pub pins: Vec<BoardPin>,
}

/// Un pin d'overlay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BoardPin {
    /// Label overlay (« D1 », « A0 »).
    pub label: String,
    pub gpio: u16,
    /// digital | analog — un pin analog ne propose que `analog_in`.
    pub kind: PinKind,
    /// Mode applied when the pin is first provisioned (never overrides a
    /// pin already configured). `None` = derived from the chip caps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_mode: Option<Mode>,
    /// Safe state applied with `default_mode` on first provisioning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_state: Option<SafeState>,
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

// ─────────────────────── Board profile v2 ───────────────────────
//
// Schema v2 of `mcu_boards.details`: a full board profile (geometry +
// integrated peripherals + positioned pin map) driving the SVG pinout
// editor. Every catalogue board is a v2 profile (the v1 overlay form is
// no longer read). Silicon capabilities (adc/strapping/input-only…) stay
// in code (`caps.rs`) and are NEVER duplicated in this JSON.

/// Board profile v2 — geometry + integrated peripherals + pin map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardProfileV2 {
    /// Board id announced by the firmware (source of `PNEX_BOARD_NAME`).
    pub board: String,
    /// Display name (UI) — e.g. « NodeMCU ESP8266 (v1.0) ».
    #[serde(default)]
    pub name: Option<String>,
    /// Silkscreen label of the shielded module — e.g. « ESP-12E ».
    #[serde(default)]
    pub chip_label: Option<String>,
    /// Physical layout (drives the SVG rendering). REQUIRED — v1/v2
    /// discriminator, see the section comment.
    pub layout: BoardLayout,
    /// Peripherals wired on the board (screens…).
    #[serde(default)]
    pub peripherals: BoardPeripherals,
    /// Pin map, positions included.
    pub pins: Vec<BoardProfilePin>,
}

/// Layout kind — only `dual_inline` today (two pin headers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoardLayoutKind {
    DualInline,
}

/// Physical geometry of the board.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoardLayout {
    pub kind: BoardLayoutKind,
    /// Pins per side (left and right headers).
    pub per_side: u16,
    /// Real PCB size, USB on top (drives the SVG proportions). Optional:
    /// profiles snapshotted before it existed fall back to a heuristic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_mm: Option<BoardSizeMm>,
}

/// PCB dimensions in millimeters: `w` across the headers, `l` along them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoardSizeMm {
    pub w: f64,
    pub l: f64,
}

/// Integrated peripherals — empty by default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoardPeripherals {
    /// Screens wired on the board (I2C OLED, SPI TFT…).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub screens: Vec<ScreenPeripheral>,
}

/// A screen wired on the board. Its pins become RESERVED for a device
/// once the user picks it (`device_registries.peripherals`). A `builtin`
/// screen is soldered on the board — always enabled, the user cannot
/// disable it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenPeripheral {
    /// Display name — « OLED 0.96" », « TFT 1.77" ».
    pub name: String,
    /// Driver kind — « ssd1306 », « st7735 »… (contract of the firmware
    /// define `PNEX_SCREEN_{KIND}`).
    pub kind: String,
    /// Bus — « i2c », « spi ».
    pub bus: String,
    /// Wiring by role: `sda`/`scl` (i2c), `sck`/`mosi`/`cs`/`dc`/`rst` (spi).
    pub pins: std::collections::BTreeMap<String, u16>,
    /// I2C address — « 0x3C ».
    #[serde(default)]
    pub addr: Option<String>,
    /// Panel size in pixels — (160, 128).
    #[serde(default)]
    pub size_px: Option<(u32, u32)>,
    /// Soldered on the board — always enabled (forced), the user cannot
    /// turn it off (default: false = external option picked per device).
    #[serde(default)]
    pub builtin: bool,
}

/// A v2 profile pin (position + board-level kind; silicon capabilities
/// live in code — `caps.rs` — never in this JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardProfilePin {
    /// Silkscreen label (« D1 », « A0 », « 3V3 »…).
    pub label: String,
    /// GPIO number — `None` for power/gnd/en pins (not configurable).
    pub gpio: Option<u16>,
    pub kind: BoardPinKind,
    /// Position on the board (drives the SVG rendering) — REQUIRED.
    pub pos: PinPos,
    /// Free-form note surfaced by the UI (conventions, wiring hints…).
    #[serde(default)]
    pub note: Option<String>,
    /// Board-wired function: mode applied when the pin is first provisioned
    /// (e.g. an onboard LED = `digital_out`). Never overrides a pin the user
    /// already configured; ignored when illegal per the chip caps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_mode: Option<Mode>,
    /// Safe state applied with `default_mode` (e.g. `high` keeps an
    /// active-low LED off at boot and on link loss).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_state: Option<SafeState>,
    /// The wired load is active LOW (writing `false` turns it on). Display
    /// metadata only: the write path never inverts values.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub active_low: bool,
    /// The board wires an external pull-down (e.g. a MOSFET gate): the chip's
    /// internal pull-up cannot raise the pad. Measured by the D122 bench.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pull_down: bool,
}

/// Board-level pin kind (header role) — silicon capabilities live in caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoardPinKind {
    Gpio,
    Power,
    Gnd,
    En,
}

/// Header side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
}

/// Pin position — side + 0-based index within that side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinPos {
    pub side: Side,
    pub index: u16,
}

/// Per-device peripherals state (`device_registries.peripherals`) — the
/// user's choice, independent of what the board COULD host. Missing jsonb
/// column or parse failure = everything disabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DevicePeripherals {
    /// Screen picked by the user (default: none).
    #[serde(default)]
    pub screen: ScreenChoice,
}

/// Screen picked at device level — wire form `{"screen": null | "ssd1306"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(from = "Option<String>", into = "Option<String>")]
pub enum ScreenChoice {
    /// No screen.
    #[default]
    None,
    /// Driver kind picked from the board profile — « ssd1306 », « st7735 ».
    Kind(String),
}

impl From<Option<String>> for ScreenChoice {
    fn from(f: Option<String>) -> Self {
        match f {
            Some(k) => ScreenChoice::Kind(k),
            None => ScreenChoice::None,
        }
    }
}

impl From<ScreenChoice> for Option<String> {
    fn from(c: ScreenChoice) -> Self {
        match c {
            ScreenChoice::None => None,
            ScreenChoice::Kind(k) => Some(k),
        }
    }
}

/// Content of `mcu_boards.details` — a v2 board profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BoardDetails {
    V2(BoardProfileV2),
}

impl BoardDetails {
    /// v2 profile, if this is one.
    pub fn v2(&self) -> Option<&BoardProfileV2> {
        match self {
            BoardDetails::V2(p) => Some(p),
        }
    }

    /// Wire id of the board (« nodemcu », « xiao_esp32c3 »…).
    pub fn board_id_str(&self) -> &str {
        match self {
            BoardDetails::V2(p) => &p.board,
        }
    }

    /// Tier-1 admission pins: gpio-carrying pins legal per the chip-caps
    /// (DigitalIn **or** AdcIn accepted — keeps the esp8266 A0 convention
    /// pin, drops flash/usb/absent gpios exposed on some headers).
    /// Peripheral-reserved gpios are NOT filtered here —
    /// callers combine with `reserved_gpios`.
    pub fn admission_pins(&self, soc: caps::Soc) -> Vec<BoardPin> {
        match self {
            BoardDetails::V2(p) => p
                .pins
                .iter()
                .filter_map(|pin| {
                    let gpio = pin.gpio?;
                    let legal = caps::validate(soc, gpio, Mode::DigitalIn, &ModeOpts::default())
                        .is_ok()
                        || caps::validate(soc, gpio, Mode::AdcIn, &ModeOpts::default()).is_ok();
                    legal.then_some(BoardPin {
                        label: pin.label.clone(),
                        gpio,
                        kind: PinKind::Digital,
                        default_mode: pin.default_mode,
                        safe_state: pin.safe_state,
                    })
                })
                .collect(),
        }
    }

    /// Gpios consumed by the ENABLED peripherals of this device.
    pub fn reserved_gpios(&self, peripherals: &DevicePeripherals) -> Vec<u16> {
        self.enabled_screens(peripherals)
            .flat_map(|s| s.pins.values().copied())
            .collect()
    }

    /// Machine token if the gpio is consumed by an enabled screen —
    /// `"board-reserved-screen:4"` (resolved client-side to
    /// `err-board-reserved-screen` with the gpio as `$value`; the screen
    /// role/bus detail is dropped from the display text).
    pub fn reserved_reason(&self, gpio: u16, peripherals: &DevicePeripherals) -> Option<String> {
        let _screen = self
            .enabled_screens(peripherals)
            .find(|s| s.pins.values().any(|g| *g == gpio))?;
        Some(format!("board-reserved-screen:{gpio}"))
    }

    /// Screens of the v2 profile effectively ENABLED for this device:
    /// every `builtin` screen (soldered — forced on) plus the single
    /// external screen chosen by the user, if any. Unknown kind → fail
    /// closed (nothing enabled).
    fn enabled_screens(
        &self,
        peripherals: &DevicePeripherals,
    ) -> impl Iterator<Item = &ScreenPeripheral> {
        let empty: &[ScreenPeripheral] = &[];
        let screens: &[ScreenPeripheral] = match self.v2() {
            Some(p) => &p.peripherals.screens,
            None => empty,
        };
        let chosen: Option<&ScreenPeripheral> = match &peripherals.screen {
            ScreenChoice::None => None,
            ScreenChoice::Kind(kind) => screens.iter().find(|s| s.kind == *kind),
        };
        screens
            .iter()
            .filter(move |s| s.builtin || Some(*s) == chosen)
    }

    /// The screen this device resolves to for the build — a `builtin`
    /// screen wins over the user's pick; otherwise the chosen external
    /// screen. Unknown kind / no screen → `None` (fail closed).
    pub fn resolved_screen(&self, peripherals: &DevicePeripherals) -> Option<&ScreenPeripheral> {
        let profile = self.v2()?;
        if let Some(b) = profile.peripherals.screens.iter().find(|s| s.builtin) {
            return Some(b);
        }
        match &peripherals.screen {
            ScreenChoice::Kind(kind) => {
                profile.peripherals.screens.iter().find(|s| s.kind == *kind)
            }
            ScreenChoice::None => None,
        }
    }

    /// Driver kind of the resolved screen (« ssd1306 »…) or `None`.
    pub fn resolved_screen_kind(&self, peripherals: &DevicePeripherals) -> Option<String> {
        self.resolved_screen(peripherals).map(|s| s.kind.clone())
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

    /// Profile v2 minimal (style NodeMCU) — helper des tests ci-dessous.
    fn v2_json() -> String {
        r#"{
            "board": "nodemcu_oled",
            "name": "NodeMCU + OLED 0.96\"",
            "chip_label": "ESP-12E",
            "layout": {"kind": "dual_inline", "per_side": 8},
            "peripherals": {
                "screens": [{
                    "name": "OLED 0.96\"", "kind": "ssd1306", "bus": "i2c",
                    "pins": {"sda": 4, "scl": 5}, "addr": "0x3C", "size_px": [128, 64]
                }]
            },
            "pins": [
                {"label": "D0", "gpio": 16, "kind": "gpio", "pos": {"side": "left", "index": 0}},
                {"label": "D1", "gpio": 5, "kind": "gpio", "pos": {"side": "right", "index": 0}},
                {"label": "D2", "gpio": 4, "kind": "gpio", "pos": {"side": "right", "index": 1}},
                {"label": "A0", "gpio": 17, "kind": "gpio", "pos": {"side": "left", "index": 1},
                 "note": "canal ADC — identifiant fil 17"},
                {"label": "3V3", "gpio": null, "kind": "power", "pos": {"side": "right", "index": 2}},
                {"label": "GND", "gpio": null, "kind": "gnd", "pos": {"side": "right", "index": 3}}
            ]
        }"#
        .to_string()
    }

    #[test]
    fn board_details_v2_parse_et_roundtrip() {
        let d: BoardDetails = serde_json::from_str(&v2_json()).unwrap();
        let profile = d.v2().expect("v2 attendu");
        assert_eq!(profile.board, "nodemcu_oled");
        assert_eq!(profile.layout.per_side, 8);
        assert_eq!(profile.peripherals.screens.len(), 1);
        assert_eq!(profile.pins.len(), 6);
        assert_eq!(d.board_id_str(), "nodemcu_oled");
        // roundtrip serde stable
        let json = serde_json::to_value(&d).unwrap();
        let back: BoardDetails = serde_json::from_value(json).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn board_details_v1_overlay_is_refused() {
        let v1 = r#"{"board":"nodemcu","pins":[{"label":"D1","gpio":5,"kind":"digital"}]}"#;
        assert!(serde_json::from_str::<BoardDetails>(v1).is_err());
    }

    #[test]
    fn board_details_sentinelle_invalide_echoue() {
        let res: Result<BoardDetails, _> = serde_json::from_str(r#"{"no":"details"}"#);
        assert!(res.is_err(), "la sentinelle ne doit pas parser");
    }

    #[test]
    fn admission_pins_exclut_flash_illegaux_garde_a0() {
        // esp8266 : A0 (17) est illégal en DigitalIn mais légal en AdcIn → gardé ;
        // 3V3/GND (gpio null) exclus.
        let d: BoardDetails = serde_json::from_str(&v2_json()).unwrap();
        let pins = d.admission_pins(caps::Soc::Esp8266);
        let gpios: Vec<u16> = pins.iter().map(|p| p.gpio).collect();
        assert!(gpios.contains(&17), "A0 doit rester admissible (AdcIn)");
        assert!(gpios.contains(&5) && gpios.contains(&4) && gpios.contains(&16));
        assert_eq!(pins.len(), 4);

        // esp32 : un pin sur la flash (gpio 6) est exclu par les chip-caps.
        let esp32 = r#"{
            "board": "devkit36", "layout": {"kind": "dual_inline", "per_side": 18},
            "pins": [
                {"label": "D6", "gpio": 6, "kind": "gpio", "pos": {"side": "left", "index": 0}},
                {"label": "D23", "gpio": 23, "kind": "gpio", "pos": {"side": "left", "index": 1}}
            ]
        }"#;
        let d: BoardDetails = serde_json::from_str(esp32).unwrap();
        let pins = d.admission_pins(caps::Soc::Esp32);
        let gpios: Vec<u16> = pins.iter().map(|p| p.gpio).collect();
        assert!(
            !gpios.contains(&6),
            "flash 6-11 doit être exclue à l'admission"
        );
        assert_eq!(gpios, vec![23]);
    }

    #[test]
    fn reservation_ecran_conditionnelle_a_l_activation() {
        let d: BoardDetails = serde_json::from_str(&v2_json()).unwrap();
        let off = DevicePeripherals {
            screen: ScreenChoice::None,
        };
        let on = DevicePeripherals {
            screen: ScreenChoice::Kind("ssd1306".into()),
        };
        // screen off → pins free
        assert_eq!(d.reserved_gpios(&off), Vec::<u16>::new());
        assert_eq!(d.reserved_reason(4, &off), None);
        // screen on → sda/scl reserved with nominal reason
        let mut gpios = d.reserved_gpios(&on);
        gpios.sort();
        assert_eq!(gpios, vec![4, 5]);
        let reason = d.reserved_reason(4, &on).unwrap();
        assert_eq!(reason, "board-reserved-screen:4");
    }

    #[test]
    fn device_peripherals_default_desactive() {
        let p = DevicePeripherals::default();
        assert_eq!(p.screen, ScreenChoice::None);
        let parsed: DevicePeripherals = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(parsed.screen, ScreenChoice::None);
        // The pre-choice boolean form is refused.
        assert!(serde_json::from_str::<DevicePeripherals>(r#"{"screen": true}"#).is_err());
        let parsed: DevicePeripherals = serde_json::from_str(r#"{"screen": null}"#).unwrap();
        assert_eq!(parsed.screen, ScreenChoice::None);
        let parsed: DevicePeripherals = serde_json::from_str(r#"{"screen": "ssd1306"}"#).unwrap();
        assert_eq!(parsed.screen, ScreenChoice::Kind("ssd1306".into()));
    }

    /// Two external screens profile (bare esp8266 style) — helper for the
    /// screen-choice tests below.
    fn v2_two_screens_json() -> String {
        r#"{
            "board": "esp8266",
            "layout": {"kind": "dual_inline", "per_side": 8},
            "peripherals": {
                "screens": [
                    {"name": "OLED 0.96\"", "kind": "ssd1306", "bus": "i2c",
                     "pins": {"sda": 4, "scl": 5}, "addr": "0x3C", "size_px": [128, 64]},
                    {"name": "TFT 1.77\"", "kind": "st7735", "bus": "spi",
                     "pins": {"sck": 14, "mosi": 13, "cs": 15, "dc": 4, "rst": 5},
                     "size_px": [160, 128]}
                ]
            },
            "pins": [
                {"label": "D1", "gpio": 5, "kind": "gpio", "pos": {"side": "right", "index": 0}},
                {"label": "D2", "gpio": 4, "kind": "gpio", "pos": {"side": "right", "index": 1}}
            ]
        }"#
        .to_string()
    }

    #[test]
    fn screen_choice_reserves_only_the_picked_one() {
        let d: BoardDetails = serde_json::from_str(&v2_two_screens_json()).unwrap();
        let none = DevicePeripherals {
            screen: ScreenChoice::None,
        };
        let oled = DevicePeripherals {
            screen: ScreenChoice::Kind("ssd1306".into()),
        };
        let tft = DevicePeripherals {
            screen: ScreenChoice::Kind("st7735".into()),
        };
        // no screen → nothing reserved
        assert_eq!(d.reserved_gpios(&none), Vec::<u16>::new());
        // OLED picked → sda/scl only
        let mut gpios = d.reserved_gpios(&oled);
        gpios.sort();
        assert_eq!(gpios, vec![4, 5]);
        // TFT picked → the 5 spi roles
        let mut gpios = d.reserved_gpios(&tft);
        gpios.sort();
        assert_eq!(gpios, vec![4, 5, 13, 14, 15]);
        // unknown kind → fail closed
        let bogus = DevicePeripherals {
            screen: ScreenChoice::Kind("sh1106".into()),
        };
        assert_eq!(d.reserved_gpios(&bogus), Vec::<u16>::new());
        assert_eq!(d.resolved_screen_kind(&bogus), None);
    }

    #[test]
    fn builtin_screen_forced_even_without_choice() {
        let mut d: BoardDetails = serde_json::from_str(&v2_json()).unwrap();
        // The v2_json helper declares the ssd1306 non-builtin — flip it to
        // cover the forced path.
        let BoardDetails::V2(ref mut p) = d;
        p.peripherals.screens[0].builtin = true;
        let none = DevicePeripherals {
            screen: ScreenChoice::None,
        };
        // builtin → always reserved even with no user choice
        let mut gpios = d.reserved_gpios(&none);
        gpios.sort();
        assert_eq!(gpios, vec![4, 5]);
        assert_eq!(d.resolved_screen_kind(&none), Some("ssd1306".to_string()));
    }

    #[test]
    fn picked_kind_serializes_as_its_name() {
        let tft = DevicePeripherals {
            screen: ScreenChoice::Kind("st7735".into()),
        };
        let json = serde_json::to_value(&tft).unwrap();
        assert_eq!(json["screen"], serde_json::json!("st7735"));
    }

    #[test]
    fn profile_pin_default_mode_flows_to_admission() {
        // ESP32-CAM style: onboard LEDs provisioned as outputs out of the
        // box; a pin without defaults keeps the chip-caps derivation.
        let json = r#"{
            "board": "cam", "layout": {"kind": "dual_inline", "per_side": 2},
            "pins": [
                {"label": "Flash LED", "gpio": 4, "kind": "gpio",
                 "pos": {"side": "left", "index": 0},
                 "default_mode": "digital_out", "safe_state": "low"},
                {"label": "Red LED", "gpio": 33, "kind": "gpio",
                 "pos": {"side": "left", "index": 1},
                 "default_mode": "digital_out", "safe_state": "high", "active_low": true},
                {"label": "IO13", "gpio": 13, "kind": "gpio",
                 "pos": {"side": "right", "index": 0}}
            ]
        }"#;
        let d: BoardDetails = serde_json::from_str(json).unwrap();
        let v2 = d.v2().expect("v2 profile");
        assert!(v2.pins[1].active_low && !v2.pins[0].active_low);
        let pins = d.admission_pins(caps::Soc::Esp32);
        let by_gpio = |g: u16| pins.iter().find(|p| p.gpio == g).unwrap().clone();
        assert_eq!(by_gpio(4).default_mode, Some(Mode::DigitalOut));
        assert_eq!(by_gpio(4).safe_state, Some(SafeState::Low));
        assert_eq!(by_gpio(33).default_mode, Some(Mode::DigitalOut));
        assert_eq!(by_gpio(33).safe_state, Some(SafeState::High));
        assert_eq!(by_gpio(13).default_mode, None);
        // Round trip: absent defaults stay absent on the wire.
        let back = serde_json::to_value(v2).unwrap();
        assert!(back["pins"][2].get("default_mode").is_none());
        assert!(back["pins"][2].get("active_low").is_none());
        assert_eq!(back["pins"][1]["active_low"], serde_json::json!(true));
    }
}
