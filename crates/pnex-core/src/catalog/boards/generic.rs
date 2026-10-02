//! Placeholder board of devices that are not a microcontroller (edge agent):
//! no SoC, no pin map, no firmware build.

use crate::catalog::CatalogBoard;

pub const BOARD: CatalogBoard = CatalogBoard {
    name: "generic",
    soc: None,
    pretty_name: None,
    pio_board: None,
    profile: None,
};
