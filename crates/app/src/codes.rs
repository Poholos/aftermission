//! The numbers ArduPilot logs in its `EV` and `ERR` records, and the names
//! the firmware gives them: the `LogEvent` and `LogErrorSubsystem`
//! enumerations of `AP_Logger/LogStructure.h`, as of the firmware of 2026.

use crate::modes::Vehicle;

/// The name of event `id`; None for a number the firmware does not define.
#[must_use]
pub fn event_name(id: u8) -> Option<&'static str> {
    Some(match id {
        10 => "Armed",
        11 => "Disarmed",
        15 => "Auto armed",
        17 => "Land complete, maybe",
        18 => "Land complete",
        19 => "Lost GPS",
        21 => "Flip start",
        22 => "Flip end",
        25 => "Home set",
        26 => "Simple mode on",
        27 => "Simple mode off",
        28 => "Not landed",
        29 => "Super simple mode on",
        30 => "Autotune initialized",
        31 => "Autotune off",
        32 => "Autotune restart",
        33 => "Autotune success",
        34 => "Autotune failed",
        35 => "Autotune reached limit",
        36 => "Autotune pilot testing",
        37 => "Autotune saved gains",
        38 => "Trim saved",
        39 => "Waypoint added",
        41 => "Fence enabled",
        42 => "Fence disabled",
        43 => "Acro trainer off",
        44 => "Acro trainer leveling",
        45 => "Acro trainer limited",
        46 => "Gripper grab",
        47 => "Gripper release",
        49 => "Parachute disabled",
        50 => "Parachute enabled",
        51 => "Parachute released",
        52 => "Landing gear deployed",
        53 => "Landing gear retracted",
        54 => "Motors emergency stopped",
        55 => "Motors emergency stop cleared",
        56 => "Motor interlock disabled",
        57 => "Motor interlock enabled",
        58 => "Rotor run-up complete",
        59 => "Rotor speed below critical",
        60 => "EKF altitude reset",
        61 => "Land canceled by pilot",
        62 => "EKF yaw reset",
        63 => "ADS-B avoidance enabled",
        64 => "ADS-B avoidance disabled",
        65 => "Proximity avoidance enabled",
        66 => "Proximity avoidance disabled",
        67 => "Primary GPS changed",
        68 => "Winch relaxed",
        69 => "Winch length control",
        70 => "Winch rate control",
        71 => "ZigZag point A stored",
        72 => "ZigZag point B stored",
        73 => "Land repositioning active",
        74 => "Standby enabled",
        75 => "Standby disabled",
        163 => "Surfaced",
        164 => "Not surfaced",
        165 => "Bottomed",
        166 => "Not bottomed",
        _ => return None,
    })
}

/// The event's name, or `Event 85` for one without.
#[must_use]
pub fn event_label(id: u8) -> String {
    event_name(id).map_or_else(|| format!("Event {id}"), str::to_string)
}

/// The subsystem an `ERR` record blames; None for a number the firmware
/// does not define.
fn subsystem_name(subsys: u8) -> Option<&'static str> {
    Some(match subsys {
        1 => "Main",
        2 => "Radio",
        3 => "Compass",
        4 => "Optical flow",
        5 => "Radio failsafe",
        6 => "Battery failsafe",
        7 => "GPS failsafe",
        8 => "GCS failsafe",
        9 => "Fence failsafe",
        10 => "Flight mode",
        11 => "GPS",
        12 => "Crash check",
        13 => "Flip",
        14 => "Autotune",
        15 => "Parachute",
        16 => "EKF check",
        17 => "EKF failsafe",
        18 => "Barometer",
        19 => "CPU",
        20 => "ADS-B failsafe",
        21 => "Terrain",
        22 => "Navigation",
        23 => "Terrain failsafe",
        24 => "EKF primary",
        25 => "Thrust loss check",
        26 => "Sensors failsafe",
        27 => "Leak failsafe",
        28 => "Pilot input",
        29 => "Vibration failsafe",
        30 => "Internal error",
        31 => "Dead reckoning failsafe",
        _ => return None,
    })
}

/// What error `code` means for `subsys`, where the code is a status: the
/// codes every subsystem shares (0 resolved, 1 failed to initialize or,
/// for a failsafe, occurred, 4 unhealthy) and the ones a few define for
/// themselves. The subsystems whose code is a payload are named in
/// [`code_text`].
fn code_name(subsys: u8, code: u8) -> Option<&'static str> {
    // the failsafes, plus the CPU, thrust loss and pilot input checks,
    // which log occurred and resolved as failsafes do
    let failsafe = matches!(subsys, 5..=9 | 17 | 19 | 20 | 23 | 25..=29 | 31);
    Some(match (subsys, code) {
        (_, 0) => "resolved",
        (_, 1) if failsafe => "occurred",
        (1, 1) => "INS delay",
        (12, 1) => "crash",
        (30, 1) => "internal errors detected",
        (_, 1) => "failed to initialize",
        (22, 4) => "failed circle init",
        (_, 4) => "unhealthy",
        (2, 2) => "late frame",
        (11 | 18, 2) => "glitch",
        (12, 2) => "loss of control",
        (13, 2) => "abandoned",
        (15, 2) => "too low",
        (15, 3) => "landed",
        (16, 2) => "bad variance",
        (18 | 26, 3) => "bad depth",
        (21, 2) => "data missing",
        (22, 2) => "failed to set destination",
        (22, 3) => "restarted RTL",
        (22, 5) => "destination outside fence",
        (22, 6) => "RTL missing rangefinder",
        _ => return None,
    })
}

/// The meaning of `code` for `subsys`: for the flight mode subsystem the
/// mode that was refused, for the EKF primary one the core that took
/// over, for the fence failsafe the breaches, for the ADS-B failsafe the
/// avoidance action; a status name otherwise, or `code 7` for one without.
fn code_text(subsys: u8, code: u8, vehicle: Vehicle) -> String {
    match (subsys, code) {
        (10, refused) => return format!("{} refused", vehicle.mode_label(refused)),
        (24, index) => return format!("core {index} is primary"),
        (9, bits) if bits != 0 => return format!("breach of {}", fence_breaches(bits)),
        (20, action) if action != 0 => return adsb_action(action),
        _ => {}
    }
    code_name(subsys, code).map_or_else(|| format!("code {code}"), str::to_string)
}

/// The fences in a breach bitmask, as the `AC_Fence` library counts them.
fn fence_breaches(bits: u8) -> String {
    let named: Vec<&str> = [
        (1, "max altitude"),
        (2, "circle"),
        (4, "polygon"),
        (8, "min altitude"),
    ]
    .into_iter()
    .filter(|(bit, _)| bits & bit != 0)
    .map(|(_, name)| name)
    .collect();
    if named.is_empty() {
        format!("bits {bits:#x}")
    } else {
        named.join(", ")
    }
}

/// The avoidance action of the ADS-B failsafe, numbered as the `MAVLink`
/// `MAV_COLLISION_ACTION` enumeration is.
fn adsb_action(action: u8) -> String {
    match action {
        1 => "report".into(),
        2 => "climb or descend".into(),
        3 => "move horizontally".into(),
        4 => "move perpendicular".into(),
        5 => "RTL".into(),
        6 => "hover".into(),
        _ => format!("action {action}"),
    }
}

/// `Compass: resolved`, or the numbers when the firmware gives no name:
/// `Subsystem 40: code 7`. `vehicle` names a refused flight mode.
#[must_use]
pub fn error_label(subsys: u8, code: u8, vehicle: Vehicle) -> String {
    let subsystem =
        subsystem_name(subsys).map_or_else(|| format!("Subsystem {subsys}"), str::to_string);
    format!("{subsystem}: {}", code_text(subsys, code, vehicle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_and_errors_read_by_name_with_numeric_fallbacks() {
        assert_eq!(event_label(10), "Armed");
        assert_eq!(event_label(62), "EKF yaw reset");
        assert_eq!(event_label(25), "Home set");
        assert_eq!(event_label(200), "Event 200");
        let copter = Vehicle::Copter;
        assert_eq!(error_label(3, 0, copter), "Compass: resolved");
        assert_eq!(error_label(5, 1, copter), "Radio failsafe: occurred");
        assert_eq!(error_label(3, 1, copter), "Compass: failed to initialize");
        assert_eq!(error_label(12, 2, copter), "Crash check: loss of control");
        assert_eq!(error_label(22, 3, copter), "Navigation: restarted RTL");
        assert_eq!(error_label(11, 4, copter), "GPS: unhealthy");
        assert_eq!(error_label(12, 1, copter), "Crash check: crash");
        assert_eq!(error_label(22, 4, copter), "Navigation: failed circle init");
        assert_eq!(error_label(18, 2, copter), "Barometer: glitch");
        assert_eq!(error_label(40, 7, copter), "Subsystem 40: code 7");
        assert_eq!(error_label(3, 9, copter), "Compass: code 9");
        // subsystems whose code is a payload, not a status
        assert_eq!(error_label(10, 5, copter), "Flight mode: Loiter refused");
        assert_eq!(
            error_label(10, 0, Vehicle::Plane),
            "Flight mode: Manual refused"
        );
        assert_eq!(error_label(24, 1, copter), "EKF primary: core 1 is primary");
        assert_eq!(error_label(9, 0, copter), "Fence failsafe: resolved");
        assert_eq!(
            error_label(9, 5, copter),
            "Fence failsafe: breach of max altitude, polygon"
        );
        assert_eq!(
            error_label(9, 32, copter),
            "Fence failsafe: breach of bits 0x20"
        );
        assert_eq!(error_label(20, 5, copter), "ADS-B failsafe: RTL");
        assert_eq!(error_label(20, 9, copter), "ADS-B failsafe: action 9");
        // the checks that log occurred and resolved like a failsafe
        assert_eq!(error_label(25, 1, copter), "Thrust loss check: occurred");
        assert_eq!(error_label(19, 1, copter), "CPU: occurred");
        assert_eq!(error_label(28, 1, copter), "Pilot input: occurred");
        assert_eq!(
            error_label(30, 1, copter),
            "Internal error: internal errors detected"
        );
        assert_eq!(error_label(26, 3, copter), "Sensors failsafe: bad depth");
    }
}
