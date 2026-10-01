//! Catalogue of the physical quantities and units offered by the CoolProp
//! flow node — shared by the editor (pickers, anchor labels) and the runtime
//! (unit conversion at the node boundary). Pure data, wasm-safe.
//!
//! Conventions:
//! - a quantity id is the CoolProp parameter name (`T`, `P`, `Hmass`…), so a
//!   config stays a valid PropsSI call; derived quantities (`Tdew`,
//!   `superheat`…) are computed by the node from several PropsSI calls;
//! - a unit converts to SI as `si = value * factor + offset`;
//! - an empty or unknown unit id means SI (legacy configs had no units).

/// Physical dimension of a quantity — selects the unit list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermoDimension {
    Temperature,
    TemperatureDelta,
    Pressure,
    SpecificEnergy,
    SpecificEntropy,
    Density,
    Fraction,
    Viscosity,
    Conductivity,
    Velocity,
    Dimensionless,
    MolarMass,
}

/// A display/conversion unit: `si = value * factor + offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThermoUnit {
    /// Stable id stored in the node config.
    pub id: &'static str,
    /// Universal symbol shown in the UI (not translated).
    pub symbol: &'static str,
    pub factor: f64,
    pub offset: f64,
}

impl ThermoUnit {
    pub fn to_si(&self, value: f64) -> f64 {
        value * self.factor + self.offset
    }

    pub fn from_si(&self, si: f64) -> f64 {
        (si - self.offset) / self.factor
    }
}

const fn unit(id: &'static str, symbol: &'static str, factor: f64, offset: f64) -> ThermoUnit {
    ThermoUnit {
        id,
        symbol,
        factor,
        offset,
    }
}

/// Standard atmosphere (Pa) — reference of the gauge pressure units.
pub const ATM_PA: f64 = 101_325.0;
const PSI_PA: f64 = 6_894.757_293_168;
const F_TO_K: f64 = 5.0 / 9.0;

const TEMPERATURE: &[ThermoUnit] = &[
    unit("degC", "°C", 1.0, 273.15),
    unit("K", "K", 1.0, 0.0),
    unit("degF", "°F", F_TO_K, 273.15 - 32.0 * F_TO_K),
];
const TEMPERATURE_DELTA: &[ThermoUnit] = &[
    unit("K", "K", 1.0, 0.0),
    unit("degC", "°C", 1.0, 0.0),
    unit("degF", "°F", F_TO_K, 0.0),
];
const PRESSURE: &[ThermoUnit] = &[
    unit("barg", "barg", 1e5, ATM_PA),
    unit("bar", "bar", 1e5, 0.0),
    unit("Pa", "Pa", 1.0, 0.0),
    unit("kPa", "kPa", 1e3, 0.0),
    unit("MPa", "MPa", 1e6, 0.0),
    unit("psig", "psig", PSI_PA, ATM_PA),
    unit("psia", "psia", PSI_PA, 0.0),
];
const SPECIFIC_ENERGY: &[ThermoUnit] = &[
    unit("kJ/kg", "kJ/kg", 1e3, 0.0),
    unit("J/kg", "J/kg", 1.0, 0.0),
];
const SPECIFIC_ENTROPY: &[ThermoUnit] = &[
    unit("kJ/kg/K", "kJ/(kg·K)", 1e3, 0.0),
    unit("J/kg/K", "J/(kg·K)", 1.0, 0.0),
];
const DENSITY: &[ThermoUnit] = &[
    unit("kg/m3", "kg/m³", 1.0, 0.0),
    unit("g/cm3", "g/cm³", 1e3, 0.0),
];
const FRACTION: &[ThermoUnit] = &[unit("frac", "0–1", 1.0, 0.0), unit("%", "%", 0.01, 0.0)];
const VISCOSITY: &[ThermoUnit] = &[
    unit("mPa.s", "mPa·s", 1e-3, 0.0),
    unit("uPa.s", "µPa·s", 1e-6, 0.0),
    unit("Pa.s", "Pa·s", 1.0, 0.0),
];
const CONDUCTIVITY: &[ThermoUnit] = &[
    unit("W/m/K", "W/(m·K)", 1.0, 0.0),
    unit("mW/m/K", "mW/(m·K)", 1e-3, 0.0),
];
const VELOCITY: &[ThermoUnit] = &[unit("m/s", "m/s", 1.0, 0.0)];
const DIMENSIONLESS: &[ThermoUnit] = &[unit("-", "–", 1.0, 0.0)];
const MOLAR_MASS: &[ThermoUnit] = &[
    unit("g/mol", "g/mol", 1e-3, 0.0),
    unit("kg/mol", "kg/mol", 1.0, 0.0),
];

impl ThermoDimension {
    /// Units offered for this dimension; the first one is the default.
    pub fn units(self) -> &'static [ThermoUnit] {
        match self {
            Self::Temperature => TEMPERATURE,
            Self::TemperatureDelta => TEMPERATURE_DELTA,
            Self::Pressure => PRESSURE,
            Self::SpecificEnergy => SPECIFIC_ENERGY,
            Self::SpecificEntropy => SPECIFIC_ENTROPY,
            Self::Density => DENSITY,
            Self::Fraction => FRACTION,
            Self::Viscosity => VISCOSITY,
            Self::Conductivity => CONDUCTIVITY,
            Self::Velocity => VELOCITY,
            Self::Dimensionless => DIMENSIONLESS,
            Self::MolarMass => MOLAR_MASS,
        }
    }

    pub fn default_unit(self) -> &'static ThermoUnit {
        &self.units()[0]
    }

    pub fn unit(self, id: &str) -> Option<&'static ThermoUnit> {
        self.units().iter().find(|u| u.id == id)
    }
}

/// How the node obtains a quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermoDerivation {
    /// Plain `PropsSI(id, …)`.
    Direct,
    /// Dew temperature at the state pressure (`Q = 1`).
    DewTemperature,
    /// Bubble temperature at the state pressure (`Q = 0`).
    BubbleTemperature,
    /// `T - T_dew` (vapour side).
    Superheat,
    /// `T_bubble - T` (liquid side).
    Subcooling,
}

/// Output grouping in the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermoGroup {
    /// State properties (also the possible inputs).
    State,
    /// Saturation-derived quantities (dew/bubble, superheat, subcooling).
    Saturation,
    /// Transport and heat-capacity properties.
    Transport,
    /// Constants of the fluid (independent of the state).
    Fluid,
}

pub const THERMO_GROUPS: &[ThermoGroup] = &[
    ThermoGroup::State,
    ThermoGroup::Saturation,
    ThermoGroup::Transport,
    ThermoGroup::Fluid,
];

/// A quantity of the catalogue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThermoQuantity {
    /// CoolProp parameter name, or a PNeX id for derived quantities.
    pub id: &'static str,
    /// Short symbol shown on the canvas anchors (`T`, `h`, `ρ`…).
    pub symbol: &'static str,
    /// Fluent key of the human label.
    pub label_key: &'static str,
    pub dimension: ThermoDimension,
    /// Accepted as a node input (part of at least one input pair).
    pub input: bool,
    pub derivation: ThermoDerivation,
    pub group: ThermoGroup,
}

const fn q(
    id: &'static str,
    symbol: &'static str,
    label_key: &'static str,
    dimension: ThermoDimension,
    input: bool,
    derivation: ThermoDerivation,
    group: ThermoGroup,
) -> ThermoQuantity {
    ThermoQuantity {
        id,
        symbol,
        label_key,
        dimension,
        input,
        derivation,
        group,
    }
}

use ThermoDerivation as Dv;
use ThermoDimension as Dm;
use ThermoGroup as G;

/// Every quantity offered by the node, in display order.
pub const THERMO_QUANTITIES: &[ThermoQuantity] = &[
    q(
        "T",
        "T",
        "thermo-q-t",
        Dm::Temperature,
        true,
        Dv::Direct,
        G::State,
    ),
    q(
        "P",
        "P",
        "thermo-q-p",
        Dm::Pressure,
        true,
        Dv::Direct,
        G::State,
    ),
    q(
        "Hmass",
        "h",
        "thermo-q-hmass",
        Dm::SpecificEnergy,
        true,
        Dv::Direct,
        G::State,
    ),
    q(
        "Smass",
        "s",
        "thermo-q-smass",
        Dm::SpecificEntropy,
        true,
        Dv::Direct,
        G::State,
    ),
    q(
        "Dmass",
        "ρ",
        "thermo-q-dmass",
        Dm::Density,
        true,
        Dv::Direct,
        G::State,
    ),
    q(
        "Q",
        "x",
        "thermo-q-q",
        Dm::Fraction,
        true,
        Dv::Direct,
        G::State,
    ),
    q(
        "Umass",
        "u",
        "thermo-q-umass",
        Dm::SpecificEnergy,
        false,
        Dv::Direct,
        G::State,
    ),
    q(
        "Tdew",
        "Tdew",
        "thermo-q-tdew",
        Dm::Temperature,
        false,
        Dv::DewTemperature,
        G::Saturation,
    ),
    q(
        "Tbubble",
        "Tbub",
        "thermo-q-tbubble",
        Dm::Temperature,
        false,
        Dv::BubbleTemperature,
        G::Saturation,
    ),
    q(
        "superheat",
        "ΔTsh",
        "thermo-q-superheat",
        Dm::TemperatureDelta,
        false,
        Dv::Superheat,
        G::Saturation,
    ),
    q(
        "subcooling",
        "ΔTsc",
        "thermo-q-subcooling",
        Dm::TemperatureDelta,
        false,
        Dv::Subcooling,
        G::Saturation,
    ),
    q(
        "Cpmass",
        "cp",
        "thermo-q-cpmass",
        Dm::SpecificEntropy,
        false,
        Dv::Direct,
        G::Transport,
    ),
    q(
        "Cvmass",
        "cv",
        "thermo-q-cvmass",
        Dm::SpecificEntropy,
        false,
        Dv::Direct,
        G::Transport,
    ),
    q(
        "viscosity",
        "μ",
        "thermo-q-viscosity",
        Dm::Viscosity,
        false,
        Dv::Direct,
        G::Transport,
    ),
    q(
        "conductivity",
        "λ",
        "thermo-q-conductivity",
        Dm::Conductivity,
        false,
        Dv::Direct,
        G::Transport,
    ),
    q(
        "speed_of_sound",
        "c",
        "thermo-q-speed-of-sound",
        Dm::Velocity,
        false,
        Dv::Direct,
        G::Transport,
    ),
    q(
        "Prandtl",
        "Pr",
        "thermo-q-prandtl",
        Dm::Dimensionless,
        false,
        Dv::Direct,
        G::Transport,
    ),
    q(
        "T_critical",
        "Tc",
        "thermo-q-t-critical",
        Dm::Temperature,
        false,
        Dv::Direct,
        G::Fluid,
    ),
    q(
        "p_critical",
        "Pc",
        "thermo-q-p-critical",
        Dm::Pressure,
        false,
        Dv::Direct,
        G::Fluid,
    ),
    q(
        "molar_mass",
        "M",
        "thermo-q-molar-mass",
        Dm::MolarMass,
        false,
        Dv::Direct,
        G::Fluid,
    ),
];

/// Input pairs offered by the node (CoolProp-supported, ordered by use).
pub const THERMO_INPUT_PAIRS: &[(&str, &str)] = &[
    ("T", "P"),
    ("P", "Hmass"),
    ("P", "Smass"),
    ("P", "Q"),
    ("T", "Q"),
    ("Hmass", "Smass"),
    ("T", "Dmass"),
    ("P", "Dmass"),
];

/// Output ids of a freshly dropped node.
pub const THERMO_DEFAULT_OUTPUTS: &[&str] = &["Hmass", "Dmass"];

pub fn thermo_quantity(id: &str) -> Option<&'static ThermoQuantity> {
    THERMO_QUANTITIES.iter().find(|q| q.id == id)
}

/// Unit of `unit_id` for quantity `quantity_id`; `None` = SI passthrough
/// (quantity outside the catalogue, empty or unknown unit id).
pub fn thermo_unit(quantity_id: &str, unit_id: &str) -> Option<&'static ThermoUnit> {
    thermo_quantity(quantity_id)?.dimension.unit(unit_id)
}

/// `value` expressed in `unit_id` → SI.
pub fn thermo_to_si(quantity_id: &str, unit_id: &str, value: f64) -> f64 {
    match thermo_unit(quantity_id, unit_id) {
        Some(u) => u.to_si(value),
        None => value,
    }
}

/// SI → `unit_id`.
pub fn thermo_from_si(quantity_id: &str, unit_id: &str, si: f64) -> f64 {
    match thermo_unit(quantity_id, unit_id) {
        Some(u) => u.from_si(si),
        None => si,
    }
}

/// Default unit id of a catalogue quantity (empty = SI for unknown ids).
pub fn thermo_default_unit(quantity_id: &str) -> &'static str {
    thermo_quantity(quantity_id)
        .map(|q| q.dimension.default_unit().id)
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9 * b.abs().max(1.0)
    }

    #[test]
    fn temperature_units_round_trip() {
        assert!(close(thermo_to_si("T", "degC", 25.0), 298.15));
        assert!(close(thermo_to_si("T", "degF", 32.0), 273.15));
        assert!(close(thermo_from_si("T", "degF", 373.15), 212.0));
        assert!(close(thermo_from_si("T", "K", 300.0), 300.0));
    }

    #[test]
    fn gauge_pressure_adds_one_atmosphere() {
        assert!(close(thermo_to_si("P", "barg", 0.0), ATM_PA));
        assert!(close(thermo_to_si("P", "barg", 2.0), 200_000.0 + ATM_PA));
        assert!(close(thermo_from_si("P", "bar", 150_000.0), 1.5));
    }

    #[test]
    fn temperature_delta_has_no_offset() {
        assert!(close(thermo_from_si("superheat", "degC", 5.0), 5.0));
        assert!(close(thermo_from_si("superheat", "degF", 5.0), 9.0));
    }

    #[test]
    fn unknown_quantity_or_unit_is_si_passthrough() {
        assert_eq!(thermo_to_si("Dmolar", "whatever", 12.5), 12.5);
        assert_eq!(thermo_to_si("T", "", 12.5), 12.5);
        assert_eq!(thermo_default_unit("Dmolar"), "");
    }

    #[test]
    fn catalogue_is_consistent() {
        let mut ids = std::collections::BTreeSet::new();
        for q in THERMO_QUANTITIES {
            assert!(ids.insert(q.id), "duplicate quantity {}", q.id);
            let mut units = std::collections::BTreeSet::new();
            for u in q.dimension.units() {
                assert!(units.insert(u.id), "duplicate unit {} in {}", u.id, q.id);
                assert!(u.factor != 0.0);
            }
        }
        for (a, b) in THERMO_INPUT_PAIRS {
            assert!(thermo_quantity(a).is_some_and(|q| q.input), "{a}");
            assert!(thermo_quantity(b).is_some_and(|q| q.input), "{b}");
        }
        for o in THERMO_DEFAULT_OUTPUTS {
            assert!(thermo_quantity(o).is_some(), "{o}");
        }
    }
}
