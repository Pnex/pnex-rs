//! Device types and capabilities of the catalog (`device_types`,
//! `device_capabilities` rows, matched by name).

/// Device type — `device_types.name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Sensor,
    Actuator,
    Mixed,
    WifiMesh,
    PowerSupply,
    Agent,
}

impl DeviceType {
    pub const ALL: [DeviceType; 6] = [
        DeviceType::Sensor,
        DeviceType::Actuator,
        DeviceType::Mixed,
        DeviceType::WifiMesh,
        DeviceType::PowerSupply,
        DeviceType::Agent,
    ];

    pub fn name(self) -> &'static str {
        match self {
            DeviceType::Sensor => "sensor",
            DeviceType::Actuator => "actuator",
            DeviceType::Mixed => "mixed",
            DeviceType::WifiMesh => "wifi_mesh",
            DeviceType::PowerSupply => "power_supply",
            DeviceType::Agent => "agent",
        }
    }
}

/// Direction of a capability — `device_capabilities.mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityMode {
    Input,
    Output,
    InputOutput,
}

/// Capability a predefined device advertises — `device_capabilities.name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    SoilMoisture,
    SoilTemperature,
    SonarLevel,
    GpsLocation,
    AirPressure,
    AirTemperature,
    AirMoisture,
    AirFlow,
    WaterFlow,
    ElectricalAcCurrent,
    ElectricalDcCurrent,
    ElectricalAcVoltage,
    ElectricalDcVoltage,
    Switch,
    Relay1,
    Relay2,
    Relay3,
    Relay4,
    Pwm1,
    Pwm2,
    Pwm3,
    Pwm4,
}

impl Capability {
    pub const ALL: [Capability; 22] = [
        Capability::SoilMoisture,
        Capability::SoilTemperature,
        Capability::SonarLevel,
        Capability::GpsLocation,
        Capability::AirPressure,
        Capability::AirTemperature,
        Capability::AirMoisture,
        Capability::AirFlow,
        Capability::WaterFlow,
        Capability::ElectricalAcCurrent,
        Capability::ElectricalDcCurrent,
        Capability::ElectricalAcVoltage,
        Capability::ElectricalDcVoltage,
        Capability::Switch,
        Capability::Relay1,
        Capability::Relay2,
        Capability::Relay3,
        Capability::Relay4,
        Capability::Pwm1,
        Capability::Pwm2,
        Capability::Pwm3,
        Capability::Pwm4,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Capability::SoilMoisture => "soil_moisture",
            Capability::SoilTemperature => "soil_temperature",
            Capability::SonarLevel => "sonar_level",
            Capability::GpsLocation => "gps_location",
            Capability::AirPressure => "air_pressure",
            Capability::AirTemperature => "air_temperature",
            Capability::AirMoisture => "air_moisture",
            Capability::AirFlow => "air_flow",
            Capability::WaterFlow => "water_flow",
            Capability::ElectricalAcCurrent => "electrical_ac_current",
            Capability::ElectricalDcCurrent => "electrical_dc_current",
            Capability::ElectricalAcVoltage => "electrical_ac_voltage",
            Capability::ElectricalDcVoltage => "electrical_dc_voltage",
            Capability::Switch => "switch",
            Capability::Relay1 => "relay_1",
            Capability::Relay2 => "relay_2",
            Capability::Relay3 => "relay_3",
            Capability::Relay4 => "relay_4",
            Capability::Pwm1 => "pwm_1",
            Capability::Pwm2 => "pwm_2",
            Capability::Pwm3 => "pwm_3",
            Capability::Pwm4 => "pwm_4",
        }
    }

    pub fn mode(self) -> CapabilityMode {
        match self {
            Capability::Relay1
            | Capability::Relay2
            | Capability::Relay3
            | Capability::Relay4
            | Capability::Pwm1
            | Capability::Pwm2
            | Capability::Pwm3
            | Capability::Pwm4 => CapabilityMode::Output,
            _ => CapabilityMode::Input,
        }
    }
}
