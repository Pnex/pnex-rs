//! Device catalog — boards, predefined devices, device types and
//! capabilities, as typed code (D121). The backend upserts it into the
//! catalog tables at boot (`tasks/seed.rs`); supporting a new board ships
//! with a release, like its chip caps and firmware.

pub mod boards;
mod pins;
pub mod products;
pub mod types;

pub use products::ShopLinks;
pub use types::{Capability, CapabilityMode, DeviceType};

use crate::boards::BoardProfileV2;
use crate::caps::Soc;

/// A physical board variant — `mcu_boards` row (`details` = profile).
#[derive(Debug, Clone, Copy)]
pub struct CatalogBoard {
    /// `mcu_boards.name` — stable key, frozen on registered devices.
    pub name: &'static str,
    /// `None` = not a microcontroller (stored as `"generic"`).
    pub soc: Option<Soc>,
    pub pretty_name: Option<&'static str>,
    /// PlatformIO board id injected at build time (`PNEX_PIO_BOARD`).
    pub pio_board: Option<&'static str>,
    /// Board profile v2 stored in `mcu_boards.details`.
    pub profile: Option<fn() -> BoardProfileV2>,
}

impl CatalogBoard {
    /// Value of `mcu_boards.soc`.
    pub fn soc_str(&self) -> &'static str {
        self.soc.map_or("generic", Soc::name)
    }
}

/// A predefined device — `predefined_devices` row.
#[derive(Debug, Clone, Copy)]
pub struct CatalogProduct {
    pub name: &'static str,
    pub pretty_name: &'static str,
    pub revision: &'static str,
    pub device_type: DeviceType,
    pub capabilities: &'static [Capability],
    /// Default board of the model.
    pub board: &'static CatalogBoard,
    pub shop: Option<ShopLinks>,
    pub description: &'static str,
    /// Fluent key resolved client-side.
    pub description_i18n: &'static str,
}

/// Every board variant, in seed order.
pub fn boards() -> &'static [CatalogBoard] {
    boards::ALL
}

/// Every predefined device, in seed order.
pub fn products() -> &'static [CatalogProduct] {
    products::ALL
}

pub fn board(name: &str) -> Option<&'static CatalogBoard> {
    boards::ALL.iter().find(|b| b.name == name)
}

pub fn product(name: &str) -> Option<&'static CatalogProduct> {
    products::ALL.iter().find(|p| p.name == name)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashSet};

    use super::*;
    use crate::boards::{BoardDetails, BoardPinKind, Side};
    use crate::caps;
    use crate::devices::{EDGE_AGENT_PREDEF, GENERIC_IO_PREDEFS};

    fn unique<'a>(names: impl Iterator<Item = &'a str>) -> bool {
        let mut seen = HashSet::new();
        names.into_iter().all(|n| seen.insert(n))
    }

    /// GPIO known to the SoC: configurable, or a flash pin shown on the
    /// header (rendered, excluded at admission).
    fn known_gpio(soc: Soc, gpio: u16) -> bool {
        match soc {
            Soc::Esp8266 => caps::is_valid_gpio(gpio) || caps::FLASH_PINS.contains(&gpio),
            Soc::Esp32 => caps::is_valid_gpio_esp32(gpio) || caps::ESP32_FLASH_PINS.contains(&gpio),
            Soc::Esp32C3 => {
                caps::is_valid_gpio_esp32c3(gpio) || caps::C3_FLASH_PINS.contains(&gpio)
            }
            Soc::Esp32C6 => {
                caps::is_valid_gpio_esp32c6(gpio) || caps::C6_FLASH_PINS.contains(&gpio)
            }
            Soc::Esp32S3 => {
                caps::is_valid_gpio_esp32s3(gpio) || caps::S3_FLASH_PINS.contains(&gpio)
            }
        }
    }

    #[test]
    fn names_are_unique() {
        assert!(unique(boards().iter().map(|b| b.name)));
        assert!(unique(products().iter().map(|p| p.name)));
        assert!(unique(DeviceType::ALL.iter().map(|t| t.name())));
        assert!(unique(Capability::ALL.iter().map(|c| c.name())));
        let ids: Vec<String> = boards()
            .iter()
            .filter_map(|b| b.profile)
            .map(|p| p().board)
            .collect();
        assert!(
            unique(ids.iter().map(String::as_str)),
            "profile board ids collide"
        );
    }

    #[test]
    fn products_reference_registered_boards() {
        for p in products() {
            assert!(
                board(p.board.name).is_some(),
                "{}: board {}",
                p.name,
                p.board.name
            );
        }
        assert!(product(EDGE_AGENT_PREDEF).is_some());
        for name in GENERIC_IO_PREDEFS {
            assert!(product(name).is_some(), "{name}");
        }
    }

    #[test]
    fn microcontroller_boards_are_buildable() {
        for b in boards() {
            let Some(soc) = b.soc else {
                assert!(b.profile.is_none() && b.pio_board.is_none(), "{}", b.name);
                continue;
            };
            assert_eq!(Soc::from_board_soc(b.soc_str()), Some(soc), "{}", b.name);
            assert!(b.pio_board.is_some(), "{}: no pio_board", b.name);
            assert!(b.profile.is_some(), "{}: no profile", b.name);
        }
    }

    #[test]
    fn profiles_are_consistent() {
        for b in boards() {
            let (Some(soc), Some(profile)) = (b.soc, b.profile) else {
                continue;
            };
            let p = profile();
            let per_side = p.layout.per_side;

            // Stored JSON parses back as a board profile.
            let json = serde_json::to_value(&p).unwrap();
            let details: BoardDetails = serde_json::from_value(json).unwrap();
            assert!(
                !details.admission_pins(soc).is_empty(),
                "{}: nothing admitted",
                b.name
            );

            // One pin per header slot: the SVG keys chips on side-index.
            let mut slots = HashSet::new();
            for pin in &p.pins {
                assert!(
                    pin.pos.index < per_side,
                    "{} {}: index out of header",
                    b.name,
                    pin.label
                );
                assert!(
                    slots.insert((pin.pos.side == Side::Left, pin.pos.index)),
                    "{} {}: slot taken",
                    b.name,
                    pin.label
                );
                match pin.kind {
                    BoardPinKind::Gpio => {
                        if let Some(g) = pin.gpio {
                            assert!(
                                known_gpio(soc, g),
                                "{} {}: gpio {g} unknown",
                                b.name,
                                pin.label
                            );
                        }
                    }
                    _ => assert!(
                        pin.gpio.is_none(),
                        "{} {}: power pin with a gpio",
                        b.name,
                        pin.label
                    ),
                }
            }
            for left in [true, false] {
                assert!(
                    slots.iter().any(|(l, _)| *l == left),
                    "{}: empty header",
                    b.name
                );
            }

            let gpios: BTreeSet<u16> = p.pins.iter().filter_map(|pin| pin.gpio).collect();
            assert_eq!(
                gpios.len(),
                p.pins.iter().filter(|pin| pin.gpio.is_some()).count(),
                "{}: gpio wired twice",
                b.name
            );
            for screen in &p.peripherals.screens {
                for (role, g) in &screen.pins {
                    assert!(
                        gpios.contains(g),
                        "{} {}: {role} on gpio {g} not on the board",
                        b.name,
                        screen.kind
                    );
                }
            }
        }
    }
}
