//! ArduPilot vehicles and the names of their flight modes.
//!
//! A `MODE` record carries the mode as a number whose meaning depends on the
//! vehicle, and a log names its vehicle only in the firmware banner it logs
//! as its first `MSG` text, such as `ArduCopter V4.7.0 (1511f271)`. The
//! tables follow the mode enumerations of the ArduPilot firmware of 2026.

/// The vehicle kind a log comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Vehicle {
    Copter,
    Plane,
    Rover,
    Sub,
    Tracker,
    Blimp,
    #[default]
    Unknown,
}

impl Vehicle {
    /// The vehicle a firmware banner names; None for text that is not one.
    #[must_use]
    pub fn from_banner(text: &str) -> Option<Vehicle> {
        let text = text.trim_start();
        let named = |prefix: &str| text.starts_with(prefix);
        Some(if named("ArduCopter") || named("APM:Copter") {
            Vehicle::Copter
        } else if named("ArduPlane") || named("APM:Plane") {
            Vehicle::Plane
        } else if named("ArduRover") || named("APM:Rover") || named("Rover V") {
            Vehicle::Rover
        } else if named("ArduSub") {
            Vehicle::Sub
        } else if named("AntennaTracker") {
            Vehicle::Tracker
        } else if named("Blimp") {
            Vehicle::Blimp
        } else {
            return None;
        })
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Vehicle::Copter => "Copter",
            Vehicle::Plane => "Plane",
            Vehicle::Rover => "Rover",
            Vehicle::Sub => "Sub",
            Vehicle::Tracker => "AntennaTracker",
            Vehicle::Blimp => "Blimp",
            Vehicle::Unknown => "unknown vehicle",
        }
    }

    /// The name of mode `number` for this vehicle; None when the firmware
    /// defines no such mode, or the vehicle is unknown.
    #[must_use]
    pub fn mode_name(self, number: u8) -> Option<&'static str> {
        let table: &[(u8, &str)] = match self {
            Vehicle::Copter => COPTER_MODES,
            Vehicle::Plane => PLANE_MODES,
            Vehicle::Rover => ROVER_MODES,
            Vehicle::Sub => SUB_MODES,
            Vehicle::Tracker => TRACKER_MODES,
            Vehicle::Blimp => BLIMP_MODES,
            Vehicle::Unknown => return None,
        };
        table
            .iter()
            .find(|(n, _)| *n == number)
            .map(|(_, name)| *name)
    }

    /// The mode's name, or `Mode 42` when it has none.
    #[must_use]
    pub fn mode_label(self, number: u8) -> String {
        self.mode_name(number)
            .map_or_else(|| format!("Mode {number}"), str::to_string)
    }
}

const COPTER_MODES: &[(u8, &str)] = &[
    (0, "Stabilize"),
    (1, "Acro"),
    (2, "AltHold"),
    (3, "Auto"),
    (4, "Guided"),
    (5, "Loiter"),
    (6, "RTL"),
    (7, "Circle"),
    (9, "Land"),
    (11, "Drift"),
    (13, "Sport"),
    (14, "Flip"),
    (15, "AutoTune"),
    (16, "PosHold"),
    (17, "Brake"),
    (18, "Throw"),
    (19, "Avoid ADSB"),
    (20, "Guided NoGPS"),
    (21, "Smart RTL"),
    (22, "FlowHold"),
    (23, "Follow"),
    (24, "ZigZag"),
    (25, "SystemID"),
    (26, "Autorotate"),
    (27, "Auto RTL"),
    (28, "Turtle"),
];

const PLANE_MODES: &[(u8, &str)] = &[
    (0, "Manual"),
    (1, "Circle"),
    (2, "Stabilize"),
    (3, "Training"),
    (4, "Acro"),
    (5, "FBWA"),
    (6, "FBWB"),
    (7, "Cruise"),
    (8, "AutoTune"),
    (10, "Auto"),
    (11, "RTL"),
    (12, "Loiter"),
    (13, "Takeoff"),
    (14, "Avoid ADSB"),
    (15, "Guided"),
    (16, "Initializing"),
    (17, "QStabilize"),
    (18, "QHover"),
    (19, "QLoiter"),
    (20, "QLand"),
    (21, "QRTL"),
    (22, "QAutotune"),
    (23, "QAcro"),
    (24, "Thermal"),
    (25, "Loiter to QLand"),
    (26, "AutoLand"),
];

const ROVER_MODES: &[(u8, &str)] = &[
    (0, "Manual"),
    (1, "Acro"),
    (3, "Steering"),
    (4, "Hold"),
    (5, "Loiter"),
    (6, "Follow"),
    (7, "Simple"),
    (8, "Dock"),
    (9, "Circle"),
    (10, "Auto"),
    (11, "RTL"),
    (12, "Smart RTL"),
    (15, "Guided"),
    (16, "Initializing"),
];

const SUB_MODES: &[(u8, &str)] = &[
    (0, "Stabilize"),
    (1, "Acro"),
    (2, "AltHold"),
    (3, "Auto"),
    (4, "Guided"),
    (7, "Circle"),
    (9, "Surface"),
    (16, "PosHold"),
    (19, "Manual"),
    (20, "Motor detect"),
    (21, "SurfTrak"),
];

const TRACKER_MODES: &[(u8, &str)] = &[
    (0, "Manual"),
    (1, "Stop"),
    (2, "Scan"),
    (3, "Servo test"),
    (4, "Guided"),
    (10, "Auto"),
    (16, "Initializing"),
];

const BLIMP_MODES: &[(u8, &str)] = &[
    (0, "Land"),
    (1, "Manual"),
    (2, "Velocity"),
    (3, "Loiter"),
    (4, "RTL"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_banner_names_the_vehicle() {
        assert_eq!(
            Vehicle::from_banner("ArduCopter V4.7.0 (1511f271)"),
            Some(Vehicle::Copter)
        );
        assert_eq!(
            Vehicle::from_banner("ArduPlane V4.7.0 (1511f271)"),
            Some(Vehicle::Plane)
        );
        assert_eq!(
            Vehicle::from_banner("ArduRover V4.7.0 (1511f271)"),
            Some(Vehicle::Rover)
        );
        assert_eq!(
            Vehicle::from_banner("APM:Copter V3.6.12"),
            Some(Vehicle::Copter)
        );
        assert_eq!(Vehicle::from_banner("ArduSub V4.5.0"), Some(Vehicle::Sub));
        assert_eq!(Vehicle::from_banner("Param space used: 231/4096"), None);
        assert_eq!(Vehicle::from_banner(""), None);
    }

    #[test]
    fn modes_are_named_per_vehicle() {
        // the same number means different modes on different vehicles
        assert_eq!(Vehicle::Copter.mode_label(0), "Stabilize");
        assert_eq!(Vehicle::Plane.mode_label(0), "Manual");
        assert_eq!(Vehicle::Rover.mode_label(10), "Auto");
        assert_eq!(Vehicle::Copter.mode_label(6), "RTL");
        assert_eq!(Vehicle::Plane.mode_label(11), "RTL");
        // gaps in the enumerations and unknown vehicles fall back to the number
        assert_eq!(Vehicle::Copter.mode_label(8), "Mode 8");
        assert_eq!(Vehicle::Copter.mode_label(200), "Mode 200");
        assert_eq!(Vehicle::Unknown.mode_label(0), "Mode 0");
        assert_eq!(Vehicle::Unknown.name(), "unknown vehicle");
    }
}
