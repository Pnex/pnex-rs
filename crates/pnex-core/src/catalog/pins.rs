//! Compact builders for board profiles: one line per header pin.

use std::collections::BTreeMap;

use crate::boards::{
    BoardLayout, BoardLayoutKind, BoardPeripherals, BoardPinKind, BoardProfilePin, BoardSizeMm,
    PinPos, ScreenPeripheral, Side,
};
use crate::proto::{Mode, SafeState};

/// Left header (as rendered, USB on top).
pub(crate) const L: Side = Side::Left;
/// Right header (as rendered, USB on top).
pub(crate) const R: Side = Side::Right;

/// Two pin headers of `per_side` pins each, PCB size in millimeters.
pub(crate) fn dual_inline(per_side: u16, w: f64, l: f64) -> BoardLayout {
    BoardLayout {
        kind: BoardLayoutKind::DualInline,
        per_side,
        size_mm: Some(BoardSizeMm { w, l }),
    }
}

pub(crate) fn screens(screens: Vec<ScreenPeripheral>) -> BoardPeripherals {
    BoardPeripherals { screens }
}

/// External 0.96" SSD1306 OLED on I2C (address 0x3C, 128x64).
pub(crate) fn oled_ssd1306(sda: u16, scl: u16) -> ScreenPeripheral {
    ScreenPeripheral {
        name: "OLED 0.96\"".into(),
        kind: "ssd1306".into(),
        bus: "i2c".into(),
        pins: BTreeMap::from([("sda".into(), sda), ("scl".into(), scl)]),
        addr: Some("0x3C".into()),
        size_px: Some((128, 64)),
        builtin: false,
    }
}

/// External 1.77" ST7735 TFT on SPI (160x128).
pub(crate) fn tft_st7735(sck: u16, mosi: u16, cs: u16, dc: u16, rst: u16) -> ScreenPeripheral {
    ScreenPeripheral {
        name: "TFT 1.77\"".into(),
        kind: "st7735".into(),
        bus: "spi".into(),
        pins: BTreeMap::from([
            ("sck".into(), sck),
            ("mosi".into(), mosi),
            ("cs".into(), cs),
            ("dc".into(), dc),
            ("rst".into(), rst),
        ]),
        addr: None,
        size_px: Some((160, 128)),
        builtin: false,
    }
}

impl ScreenPeripheral {
    /// Soldered on the board: always enabled, the user cannot turn it off.
    pub(crate) fn builtin(mut self) -> Self {
        self.builtin = true;
        self
    }
}

fn pin(
    label: &str,
    gpio: Option<u16>,
    kind: BoardPinKind,
    side: Side,
    index: u16,
) -> BoardProfilePin {
    BoardProfilePin {
        label: label.into(),
        gpio,
        kind,
        pos: PinPos { side, index },
        note: None,
        default_mode: None,
        safe_state: None,
        active_low: false,
        pull_down: false,
    }
}

/// Configurable GPIO (admission still filters it through the chip caps).
pub(crate) fn io(label: &str, gpio: u16, side: Side, index: u16) -> BoardProfilePin {
    pin(label, Some(gpio), BoardPinKind::Gpio, side, index)
}

/// GPIO header pin rendered but never provisioned (taken by the module).
pub(crate) fn unwired(label: &str, side: Side, index: u16) -> BoardProfilePin {
    pin(label, None, BoardPinKind::Gpio, side, index)
}

pub(crate) fn power(label: &str, side: Side, index: u16) -> BoardProfilePin {
    pin(label, None, BoardPinKind::Power, side, index)
}

pub(crate) fn gnd(label: &str, side: Side, index: u16) -> BoardProfilePin {
    pin(label, None, BoardPinKind::Gnd, side, index)
}

pub(crate) fn en(label: &str, side: Side, index: u16) -> BoardProfilePin {
    pin(label, None, BoardPinKind::En, side, index)
}

impl BoardProfilePin {
    pub(crate) fn note(mut self, note: &str) -> Self {
        self.note = Some(note.into());
        self
    }

    pub(crate) fn default_mode(mut self, mode: Mode) -> Self {
        self.default_mode = Some(mode);
        self
    }

    pub(crate) fn safe_state(mut self, state: SafeState) -> Self {
        self.safe_state = Some(state);
        self
    }

    pub(crate) fn active_low(mut self) -> Self {
        self.active_low = true;
        self
    }

    pub(crate) fn pull_down(mut self) -> Self {
        self.pull_down = true;
        self
    }
}
