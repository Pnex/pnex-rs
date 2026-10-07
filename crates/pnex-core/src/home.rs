//! Home cards of the dashboards (D138, `docs/architecture/home-dashboards.md`):
//! one widget of type [`HOME_WIDGET_TYPE`] = one composite card (light,
//! thermostat, shutter…) driving **N org controls named by role** and
//! reading **sources named by role** (`SourceRef.role`). Pure and wasm-safe.
//!
//! Each card kind has a static table of its control roles (kind + default
//! domain) and source roles. Required control roles always exist; optional
//! ones exist when the widget declares their domain in
//! [`HomeCardOptions::specs`]. The server provisions one control per active
//! role at save (D131 school, item id `{widget}.{role}`), applies the
//! declared domain and writes the bound ids back into
//! [`HomeCardOptions::controls`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ui_control::{ColorMode, ControlKind, ControlOption, ControlRef, ControlSpec};

/// Widget type of every home card.
pub const HOME_WIDGET_TYPE: &str = "home_card";

/// Composite card kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeCard {
    Light,
    Thermostat,
    Fan,
    Cover,
    Gate,
    Lock,
    Alarm,
    Scene,
    Irrigation,
    Binary,
    ThermoHygro,
    AirQuality,
    Power,
    Meter,
    EnergyFlow,
    Appliance,
    Clock,
    /// Weather of the `weather` flow node, read from org memory (D140).
    Weather,
}

/// One control a card drives.
#[derive(Debug, Clone, Copy)]
pub struct ControlRole {
    pub role: &'static str,
    pub kind: ControlKind,
    /// Required roles always exist; optional ones are toggled by declaring
    /// their domain.
    pub required: bool,
}

/// One value a card reads.
#[derive(Debug, Clone, Copy)]
pub struct SourceRole {
    pub role: &'static str,
    pub required: bool,
}

// Macros (not const fns): struct literals are promoted to 'static.
macro_rules! ctl {
    ($role:expr, $kind:expr, $required:expr) => {
        ControlRole {
            role: $role,
            kind: $kind,
            required: $required,
        }
    };
}

macro_rules! src {
    ($role:expr, $required:expr) => {
        SourceRole {
            role: $role,
            required: $required,
        }
    };
}

/// Source roles of the weather card: fields of the current payload, then
/// `d{n}_*` fields of the daily payload (role = memory field name).
pub const WEATHER_CURRENT_ROLES: &[&str] = &[
    "temperature",
    "condition_code",
    "is_day",
    "feels_like",
    "humidity",
    "wind_speed",
];
/// Forecast days shown by the weather card.
pub const WEATHER_CARD_DAYS: usize = 5;

const WEATHER_SOURCE_ROLES: &[SourceRole] = &[
    src!("temperature", true),
    src!("condition_code", false),
    src!("is_day", false),
    src!("feels_like", false),
    src!("humidity", false),
    src!("wind_speed", false),
    src!("d0_t_min", false),
    src!("d0_t_max", false),
    src!("d0_condition_code", false),
    src!("d1_t_min", false),
    src!("d1_t_max", false),
    src!("d1_condition_code", false),
    src!("d2_t_min", false),
    src!("d2_t_max", false),
    src!("d2_condition_code", false),
    src!("d3_t_min", false),
    src!("d3_t_max", false),
    src!("d3_condition_code", false),
    src!("d4_t_min", false),
    src!("d4_t_max", false),
    src!("d4_condition_code", false),
];

/// Variants of the binary sensor card (icon and on/off wording).
pub const BINARY_VARIANTS: &[&str] = &[
    "door", "window", "motion", "presence", "smoke", "leak", "co", "generic",
];

impl HomeCard {
    pub const ALL: [HomeCard; 18] = [
        HomeCard::Light,
        HomeCard::Thermostat,
        HomeCard::Fan,
        HomeCard::Cover,
        HomeCard::Gate,
        HomeCard::Lock,
        HomeCard::Alarm,
        HomeCard::Scene,
        HomeCard::Irrigation,
        HomeCard::Binary,
        HomeCard::ThermoHygro,
        HomeCard::AirQuality,
        HomeCard::Power,
        HomeCard::Meter,
        HomeCard::EnergyFlow,
        HomeCard::Appliance,
        HomeCard::Clock,
        HomeCard::Weather,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            HomeCard::Light => "light",
            HomeCard::Thermostat => "thermostat",
            HomeCard::Fan => "fan",
            HomeCard::Cover => "cover",
            HomeCard::Gate => "gate",
            HomeCard::Lock => "lock",
            HomeCard::Alarm => "alarm",
            HomeCard::Scene => "scene",
            HomeCard::Irrigation => "irrigation",
            HomeCard::Binary => "binary",
            HomeCard::ThermoHygro => "thermo_hygro",
            HomeCard::AirQuality => "air_quality",
            HomeCard::Power => "power",
            HomeCard::Meter => "meter",
            HomeCard::EnergyFlow => "energy_flow",
            HomeCard::Appliance => "appliance",
            HomeCard::Clock => "clock",
            HomeCard::Weather => "weather",
        }
    }

    pub fn parse(s: &str) -> Option<HomeCard> {
        HomeCard::ALL.into_iter().find(|c| c.as_str() == s)
    }

    /// Control roles, display order.
    pub fn control_roles(self) -> &'static [ControlRole] {
        use ControlKind::*;
        match self {
            HomeCard::Light => &[
                ctl!("power", Switch, true),
                ctl!("level", Slider, false),
                ctl!("color", Color, false),
            ],
            HomeCard::Thermostat => &[ctl!("setpoint", Stepper, true), ctl!("mode", Select, false)],
            HomeCard::Fan => &[ctl!("power", Switch, true), ctl!("speed", Select, false)],
            HomeCard::Cover => &[
                ctl!("command", Command, true),
                ctl!("position", Slider, false),
            ],
            HomeCard::Gate => &[ctl!("command", Command, true)],
            HomeCard::Lock => &[ctl!("command", Command, true)],
            HomeCard::Alarm => &[ctl!("mode", Select, true)],
            HomeCard::Scene => &[ctl!("trigger", Button, true)],
            HomeCard::Irrigation => &[ctl!("power", Switch, true), ctl!("duration", Number, false)],
            HomeCard::Appliance => &[ctl!("power", Switch, false)],
            _ => &[],
        }
    }

    /// Source roles, display order.
    pub fn source_roles(self) -> &'static [SourceRole] {
        match self {
            HomeCard::Light => &[src!("state", false)],
            HomeCard::Thermostat => &[src!("current", false), src!("humidity", false)],
            HomeCard::Fan => &[src!("state", false)],
            HomeCard::Cover => &[src!("position", false)],
            HomeCard::Gate => &[src!("state", false)],
            HomeCard::Lock => &[src!("state", false)],
            HomeCard::Alarm => &[src!("state", false)],
            HomeCard::Scene => &[],
            HomeCard::Irrigation => &[src!("state", false), src!("moisture", false)],
            HomeCard::Binary => &[src!("state", true)],
            HomeCard::ThermoHygro => &[src!("temperature", true), src!("humidity", false)],
            HomeCard::AirQuality => &[src!("co2", true), src!("voc", false), src!("pm25", false)],
            HomeCard::Power => &[src!("power", true)],
            HomeCard::Meter => &[src!("energy", true)],
            HomeCard::EnergyFlow => &[
                src!("grid", true),
                src!("solar", false),
                src!("battery", false),
                src!("battery_soc", false),
                src!("home", false),
            ],
            HomeCard::Appliance => &[
                src!("status", true),
                src!("progress", false),
                src!("remaining", false),
            ],
            HomeCard::Clock => &[],
            HomeCard::Weather => WEATHER_SOURCE_ROLES,
        }
    }

    pub fn control_role(self, role: &str) -> Option<&'static ControlRole> {
        self.control_roles().iter().find(|r| r.role == role)
    }

    pub fn source_role(self, role: &str) -> Option<&'static SourceRole> {
        self.source_roles().iter().find(|r| r.role == role)
    }

    /// Default domain of a role (the card's usual values).
    pub fn default_spec(self, role: &str) -> Option<ControlSpec> {
        let r = self.control_role(role)?;
        let mut spec = ControlSpec::new(r.kind);
        let opt = |value: f64, key: &str, icon: Option<&str>| ControlOption {
            value,
            key: key.to_string(),
            label: None,
            icon: icon.map(str::to_string),
        };
        match (self, role) {
            (HomeCard::Light, "level") | (HomeCard::Cover, "position") => {
                spec.unit = Some("%".into());
            }
            (HomeCard::Light, "color") => {
                spec.color = Some(ColorMode::Rgb);
            }
            (HomeCard::Thermostat, "setpoint") => {
                spec.min = Some(5.0);
                spec.max = Some(30.0);
                spec.step = Some(0.5);
                spec.unit = Some("°C".into());
            }
            (HomeCard::Thermostat, "mode") => {
                spec.options = vec![
                    opt(0.0, "off", Some("home-power")),
                    opt(1.0, "heat", Some("home-flame")),
                    opt(2.0, "cool", Some("home-snowflake")),
                    opt(3.0, "auto", None),
                ];
            }
            (HomeCard::Fan, "speed") => {
                spec.options = vec![
                    opt(1.0, "low", None),
                    opt(2.0, "medium", None),
                    opt(3.0, "high", None),
                ];
            }
            (HomeCard::Gate, "command") => {
                spec.options = vec![
                    opt(1.0, "open", Some("home-gate")),
                    opt(0.0, "close", Some("home-arrow-down")),
                ];
                spec.confirm = true;
            }
            (HomeCard::Lock, "command") => {
                spec.options = vec![
                    opt(1.0, "lock", Some("home-lock")),
                    opt(0.0, "unlock", Some("home-unlock")),
                ];
                spec.confirm = true;
            }
            (HomeCard::Alarm, "mode") => {
                spec.options = vec![
                    opt(0.0, "disarm", Some("home-shield")),
                    opt(1.0, "arm_home", Some("home-house")),
                    opt(2.0, "arm_away", Some("home-shield-check")),
                    opt(3.0, "arm_night", Some("home-moon")),
                ];
                spec.confirm = true;
            }
            (HomeCard::Irrigation, "duration") => {
                spec.min = Some(1.0);
                spec.max = Some(240.0);
                spec.step = Some(1.0);
                spec.unit = Some("min".into());
            }
            _ => {}
        }
        Some(spec)
    }

    /// Default card icon (home icon catalog).
    pub fn default_icon(self, variant: Option<&str>) -> &'static str {
        match self {
            HomeCard::Light => "home-bulb",
            HomeCard::Thermostat => "home-thermostat",
            HomeCard::Fan => "home-fan",
            HomeCard::Cover => "home-shutter",
            HomeCard::Gate => "home-gate",
            HomeCard::Lock => "home-lock",
            HomeCard::Alarm => "home-shield",
            HomeCard::Scene => "home-scene",
            HomeCard::Irrigation => "home-sprinkler",
            HomeCard::Binary => match variant.unwrap_or("generic") {
                "door" => "home-door",
                "window" => "home-window",
                "motion" => "home-motion",
                "presence" => "home-person",
                "smoke" => "home-smoke",
                "leak" => "home-leak",
                "co" => "home-co",
                _ => "home-check",
            },
            HomeCard::ThermoHygro => "home-thermometer",
            HomeCard::AirQuality => "home-leaf",
            HomeCard::Power => "home-bolt",
            HomeCard::Meter => "home-meter",
            HomeCard::EnergyFlow => "home-solar",
            HomeCard::Appliance => "home-washer",
            HomeCard::Clock => "home-clock",
            HomeCard::Weather => "home-partly-cloudy",
        }
    }

    /// Mobile width by default: compact cards take half a row.
    pub fn default_span(self) -> u8 {
        match self {
            HomeCard::Binary
            | HomeCard::Scene
            | HomeCard::ThermoHygro
            | HomeCard::Power
            | HomeCard::Lock
            | HomeCard::Gate => 1,
            _ => 2,
        }
    }
}

/// Options of a [`HOME_WIDGET_TYPE`] widget (`WidgetOptions.home`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HomeCardOptions {
    pub card: HomeCard,
    /// Control bound to each active role (written by the server at save).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub controls: BTreeMap<String, ControlRef>,
    /// Domain declared per role. An optional role is active iff present.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub specs: BTreeMap<String, ControlSpec>,
    /// Binary sensor variant ([`BINARY_VARIANTS`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// The state source reports "on" when its value is ≥ this threshold
    /// (default 0.5: 0 / 1 sensors).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_above: Option<f64>,
}

impl HomeCardOptions {
    /// New card with its required roles declared at their default domain.
    pub fn new(card: HomeCard) -> Self {
        let specs = card
            .control_roles()
            .iter()
            .filter(|r| r.required)
            .filter_map(|r| card.default_spec(r.role).map(|s| (r.role.to_string(), s)))
            .collect();
        Self {
            card,
            controls: BTreeMap::new(),
            specs,
            variant: (card == HomeCard::Binary).then(|| "door".to_string()),
            on_above: None,
        }
    }

    /// Active control roles: required ones plus the declared optional ones.
    pub fn active_roles(&self) -> Vec<&'static ControlRole> {
        self.card
            .control_roles()
            .iter()
            .filter(|r| r.required || self.specs.contains_key(r.role))
            .collect()
    }

    /// Domain of an active role: declared, else default.
    pub fn spec_of(&self, role: &str) -> Option<ControlSpec> {
        self.specs
            .get(role)
            .cloned()
            .or_else(|| self.card.default_spec(role))
    }

    pub fn control_of(&self, role: &str) -> Option<Uuid> {
        self.controls.get(role).map(|c| c.control_id)
    }

    /// Is the value `v` of a binary state "on"?
    pub fn is_on(&self, v: f64) -> bool {
        v >= self.on_above.unwrap_or(0.5)
    }
}

/// Item id of the control of `role` in the D131 origin (`w-0001.setpoint`).
pub fn role_item_id(widget_id: &str, role: &str) -> String {
    format!("{widget_id}.{role}")
}

/// Label of the control a home card role provisions: `Room · Card · role`.
/// The room (mobile section title) tells apart homonym cards of a template
/// (two "Ceiling light"); the role stays its key, language neutral, and the
/// UI shows it under its localized name (O30). Empty parts are skipped.
pub fn role_control_label(room: &str, card_title: &str, role: &str) -> String {
    [room.trim(), card_title.trim(), role]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Structural check of a home card (save validation). Returns machine
/// codes with a canonical English message.
pub fn check_home_card(
    opts: &HomeCardOptions,
    source_roles: &[&str],
) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for role in opts.specs.keys().chain(opts.controls.keys()) {
        if opts.card.control_role(role).is_none() {
            out.push((
                "home_unknown_role",
                format!("unknown control role \"{role}\" for this card"),
            ));
        }
    }
    for (role, spec) in &opts.specs {
        let Some(r) = opts.card.control_role(role) else {
            continue;
        };
        if spec.kind != r.kind {
            out.push((
                "home_role_kind",
                format!("role \"{role}\" expects a {} control", r.kind.as_str()),
            ));
        } else if let Some((code, message)) = spec.check() {
            out.push((code, message));
        }
    }
    for role in source_roles {
        if opts.card.source_role(role).is_none() {
            out.push((
                "home_unknown_source",
                format!("unknown source role \"{role}\" for this card"),
            ));
        }
    }
    for r in opts.card.source_roles().iter().filter(|r| r.required) {
        if !source_roles.contains(&r.role) {
            out.push((
                "home_source_missing",
                format!("the card needs its \"{}\" source", r.role),
            ));
        }
    }
    if let Some(v) = &opts.variant {
        if opts.card != HomeCard::Binary || !BINARY_VARIANTS.contains(&v.as_str()) {
            out.push(("home_bad_variant", "invalid card variant".into()));
        }
    }
    if opts.on_above.is_some_and(|v| !v.is_finite()) {
        out.push((
            "home_bad_threshold",
            "the on threshold must be finite".into(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn role_control_label_names_room_card_and_role() {
        assert_eq!(
            super::role_control_label("Living room", "Ceiling light", "power"),
            "Living room · Ceiling light · power"
        );
        assert_eq!(
            super::role_control_label(" ", "Ceiling light", "power"),
            "Ceiling light · power"
        );
    }

    use super::*;

    #[test]
    fn every_default_spec_is_valid_and_of_its_role_kind() {
        for card in HomeCard::ALL {
            assert_eq!(HomeCard::parse(card.as_str()), Some(card));
            for r in card.control_roles() {
                let spec = card.default_spec(r.role).expect("default spec");
                assert_eq!(spec.kind, r.kind, "{}.{}", card.as_str(), r.role);
                assert!(spec.check().is_none(), "{}.{}", card.as_str(), r.role);
            }
            let fresh = HomeCardOptions::new(card);
            let sources: Vec<&str> = card
                .source_roles()
                .iter()
                .filter(|r| r.required)
                .map(|r| r.role)
                .collect();
            assert!(check_home_card(&fresh, &sources).is_empty(), "{card:?}");
            assert!(crate::valid_symbol_id(card.default_icon(None)));
        }
        for v in BINARY_VARIANTS {
            assert!(crate::valid_symbol_id(
                HomeCard::Binary.default_icon(Some(v))
            ));
        }
    }

    #[test]
    fn optional_roles_are_active_once_declared() {
        let mut light = HomeCardOptions::new(HomeCard::Light);
        let roles =
            |o: &HomeCardOptions| o.active_roles().iter().map(|r| r.role).collect::<Vec<_>>();
        assert_eq!(roles(&light), vec!["power"]);
        light.specs.insert(
            "level".into(),
            HomeCard::Light.default_spec("level").unwrap(),
        );
        assert_eq!(roles(&light), vec!["power", "level"]);
    }

    #[test]
    fn bad_cards_are_reported() {
        let mut t = HomeCardOptions::new(HomeCard::Thermostat);
        t.specs
            .insert("bogus".into(), ControlSpec::new(ControlKind::Switch));
        t.specs
            .insert("mode".into(), ControlSpec::new(ControlKind::Switch));
        let codes: Vec<&str> = check_home_card(&t, &["nope"])
            .into_iter()
            .map(|c| c.0)
            .collect();
        assert!(codes.contains(&"home_unknown_role"));
        assert!(codes.contains(&"home_role_kind"));
        assert!(codes.contains(&"home_unknown_source"));
        let b = HomeCardOptions::new(HomeCard::Binary);
        let codes: Vec<&str> = check_home_card(&b, &[]).into_iter().map(|c| c.0).collect();
        assert_eq!(codes, vec!["home_source_missing"]);
        let mut l = HomeCardOptions::new(HomeCard::Light);
        l.variant = Some("door".into());
        assert_eq!(check_home_card(&l, &[])[0].0, "home_bad_variant");
    }

    #[test]
    fn binary_state_threshold() {
        let mut b = HomeCardOptions::new(HomeCard::Binary);
        assert!(b.is_on(1.0) && !b.is_on(0.0));
        b.on_above = Some(20.0);
        assert!(!b.is_on(10.0) && b.is_on(25.0));
        assert_eq!(role_item_id("w-0001", "setpoint"), "w-0001.setpoint");
    }
}
