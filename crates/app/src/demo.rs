//! The demo flights: invented quadcopter surveys of about eight and a
//! half minutes each, written as logs through dflog's writer when asked,
//! so the repository holds no binary and the page downloads none.
//! Every value comes from the formulas here and a seeded noise generator;
//! nothing was read from a real log.
//!
//! Flight A, the demo, in seconds since boot: the tables and the
//! parameter dump; GPS acquiring a fix while the copter sits on a field in
//! Grundy County, Iowa; at 45 s arming in Loiter and a takeoff to 50 m; at
//! 80 s Auto over six survey legs of 400 m, 40 m apart, in a crosswind,
//! with a speed change at 250 s and a GPS fault at 300 s; then RTL, Land,
//! and the disarm. The message types, formats, units and multipliers are
//! ArduPilot 4.7.0's, as are the parameters' names, units and defaults,
//! so the app reads it as it reads a real log from that firmware.
//!
//! Flight B, which the demo pair compares with it, is a second sortie
//! from the same field two hours later: armed 12 s sooner after boot, it
//! flies out to a survey laid 250 m north of A's at the speed A ends on,
//! in a stronger wind, with no fault, on retuned rate gains and the hover
//! throttle and ground pressure the autopilot learned since. For the
//! retune, PID logging is back on, which A had turned off, so B logs the
//! rate controllers' and the vertical acceleration's PID loops, `PIDR`,
//! `PIDP`, `PIDY` and `PIDA`, which A does not.
//!
//! The flight path is integrated with sums, products, quotients and
//! square roots only, which every platform rounds the same way, so the
//! record counts
//! and the event times are the same everywhere. The logged values also
//! go through the platform's sine, cosine and arc tangent, whose last
//! bit can differ, so the bytes may not. Within each 10 ms step the
//! script's records come first and the sensor streams after them, so
//! the file is in time order, as a real log is.

use std::f64::consts::TAU;

use dflog::access::Value;
use dflog::write::LogWriter;

/// What the opened log is called.
pub const NAME: &str = "demo.bin";
/// What the second sortie, flight B, is called: no slot's letter in it,
/// since a swap opens it as A.
pub const NAME_SECOND: &str = "demo2.bin";
/// What the status line says while the log is generated and indexed.
pub const LABEL: &str = "Generating the demo log";
/// What it says while the pair is.
pub const PAIR_LABEL: &str = "Generating the demo pair";

/// Home: a field about 5 km south of Morrison in Grundy County, Iowa,
/// clear of the airports, towers and wind farms on the sectional chart.
const HOME_LAT: f64 = 42.2990;
const HOME_LON: f64 = -92.6790;
/// The field's elevation, meters above sea level.
const HOME_ALT: f64 = 306.0;
/// Meters per degree of latitude.
const M_PER_DEG: f64 = 111_320.0;
/// The GPS week and the milliseconds into it at boot: 2026-05-14, 14:30
/// UTC, with GPS time's 18 seconds ahead of UTC.
const GPS_WEEK: u64 = 2418;
const GPS_MS_AT_BOOT: u64 = 397_818_000;
const G: f64 = 9.806_65;

/// The simulation's step: 100 Hz.
const DT: f64 = 0.01;
const TICK_US: u64 = 10_000;
/// A safety net for the loop: no flight of this script runs this long.
const MAX_TICKS: u64 = ticks(900.0);
/// The autopilot's main loop rate, Hz, `SCHED_LOOP_RATE`: `ANG` logs its
/// period.
const LOOP_RATE: f64 = 400.0;

/// Meters per second squared, the copter's horizontal acceleration.
const ACCEL: f64 = 1.5;
/// Its vertical acceleration.
const VZ_ACCEL: f64 = 1.0;
/// Climb and descent rates in Loiter, Auto and RTL, meters per second.
const CLIMB_UP: f64 = 2.5;
const CLIMB_DOWN: f64 = 1.5;
/// The Land mode's rates above and below 10 m.
const LAND_FAST: f64 = 1.5;
const LAND_SLOW: f64 = 0.5;
/// The survey's height above home and the height at which the pilot
/// switches RTL's descent to Land.
const SURVEY_ALT: f64 = 50.0;
const LAND_SWITCH_ALT: f64 = 20.0;
/// How close to a waypoint counts as reaching it.
const WP_RADIUS: f64 = 1.0;
/// Seconds: close to its target the copter closes on it no faster than
/// the distance over this, so it settles there rather than overshooting
/// by a hair each step and dithering across it. The cap binds below
/// `2.0 * ACCEL * SETTLE * SETTLE`, 0.75 m, inside `WP_RADIUS` (checked
/// below), so it shapes only the hover over home.
const SETTLE: f64 = 0.5;
const _: () = assert!(2.0 * ACCEL * SETTLE * SETTLE < WP_RADIUS);
/// Degrees per second, the turn toward a new heading.
const YAW_RATE: f64 = 60.0;
/// The lag of the attitude behind the lean the acceleration asks for.
const ATT_LAG: f64 = 0.15;
/// The throttle that hovers, and the one the armed motors idle at.
const HOVER: f64 = 0.42;
const SPIN_ARM: f64 = 0.10;
/// The battery's capacity, mAh.
const CAPACITY: f64 = 5200.0;

/// The survey: six legs east and west, 40 m apart, flown as a lawnmower.
const LEG: f64 = 400.0;
const SPACING: f64 = 40.0;
const WAYPOINTS: usize = 12;

/// Copter mode numbers and mode-change reasons.
const STABILIZE: u64 = 0;
const AUTO: u64 = 3;
const LOITER: u64 = 5;
const RTL: u64 = 6;
const LAND: u64 = 9;
const REASON_RC: u64 = 1;
const REASON_MISSION_END: u64 = 8;
const REASON_INITIALIZED: u64 = 26;
/// The GPS error subsystem and its unhealthy and resolved codes.
const GPS_SUBSYSTEM: u64 = 11;
const CODE_UNHEALTHY: u64 = 4;
const CODE_RESOLVED: u64 = 0;
/// Event ids.
const EV_ARMED: u64 = 10;
const EV_DISARMED: u64 = 11;
const EV_AUTO_ARMED: u64 = 15;
const EV_LAND_COMPLETE: u64 = 18;
const EV_LOST_GPS: u64 = 19;
const EV_HOME_SET: u64 = 25;
const EV_NOT_LANDED: u64 = 28;

/// The script, in ticks.
const T_PARAMS: u64 = ticks(1.0);
const T_PARAMS_END: u64 = T_PARAMS + PARAMS.len() as u64;
const T_BOOT_MODE: u64 = ticks(2.2);
const T_FIX_2D: u64 = ticks(6.0);
const T_FIX_3D: u64 = ticks(12.0);
const T_EKF_GPS: u64 = ticks(20.0);
const T_ORIGIN: u64 = ticks(22.0);
const T_ARM: u64 = ticks(45.0);
const T_TAKEOFF: u64 = ticks(50.0);
const T_AUTO: u64 = ticks(80.0);
const T_SPEED: u64 = ticks(250.0);
const T_FAULT: u64 = ticks(300.0);
const FAULT_TICKS: u64 = ticks(2.0);
/// After touchdown: the land event, the disarm, the mode switch, the end.
const AFTER_LAND_EVENT: u64 = ticks(0.5);
const AFTER_LAND_DISARM: u64 = ticks(2.0);
const AFTER_LAND_MODE: u64 = ticks(5.0);
const AFTER_LAND_END: u64 = ticks(10.0);

/// The message types: name, format, labels, units, multipliers, as
/// 4.7.0 defines them, in its libraries' `LogStructure.h` and, for
/// `CTUN`, Copter's `Log.cpp`. Each is defined with its place in the
/// list, from 1, as its id.
const TYPES: [(&str, &str, &str, &str, &str); 19] = [
    ("MSG", "QBBZ", "TimeUS,ID,Seq,Message", "s---", "F---"),
    ("UNIT", "QbZ", "TimeUS,Id,Label", "s--", "F--"),
    ("MULT", "Qbd", "TimeUS,Id,Mult", "s--", "F--"),
    (
        "FMTU",
        "QBNN",
        "TimeUS,FmtType,UnitIds,MultIds",
        "s---",
        "F---",
    ),
    ("PARM", "QNff", "TimeUS,Name,Value,Default", "s---", "F---"),
    ("MODE", "QMBB", "TimeUS,Mode,ModeNum,Rsn", "s---", "F---"),
    ("EV", "QB", "TimeUS,Id", "s-", "F-"),
    ("ERR", "QBB", "TimeUS,Subsys,ECode", "s--", "F--"),
    (
        "GPS",
        "QBBIHBcLLeffffB",
        "TimeUS,I,Status,GMS,GWk,NSats,HDop,Lat,Lng,Alt,Spd,GCrs,VZ,Yaw,U",
        "s#-s-S-DUmnhnh-",
        "F--C-0BGGB000--",
    ),
    (
        "POS",
        "QLLfff",
        "TimeUS,Lat,Lng,Alt,RelHomeAlt,RelOriginAlt",
        "sDUmmm",
        "FGG000",
    ),
    (
        "ATT",
        "QffffffB",
        "TimeUS,DesRoll,Roll,DesPitch,Pitch,DesYaw,Yaw,AEKF",
        "sddddhh-",
        "F000000-",
    ),
    (
        "ANG",
        "Qfffffff",
        "TimeUS,DesRoll,Roll,DesPitch,Pitch,DesYaw,Yaw,Dt",
        "sddddhhs",
        "F0000000",
    ),
    // ADes and A keep the 0.01 of the centimeters they were logged in
    // before 4.7.0, which logs them in m/s/s: the app shows them a
    // hundredth of their size, as it does in a real 4.7.0 log
    (
        "RATE",
        "Qfffffffffffff",
        "TimeUS,RDes,R,ROut,PDes,P,POut,YDes,Y,YOut,ADes,A,AOut,AOutSlew",
        "skk-kk-kk-oo--",
        "F?????????BB--",
    ),
    (
        "IMU",
        "QBffffffIIfBBHH",
        "TimeUS,I,GyrX,GyrY,GyrZ,AccX,AccY,AccZ,EG,EA,T,GH,AH,GHz,AHz",
        "s#EEEooo--O--zz",
        "F-000000-----00",
    ),
    (
        "BARO",
        "QBfffcfIffBf",
        "TimeUS,I,Alt,AltAMSL,Press,Temp,CRt,SMS,Offset,GndTemp,H,CPress",
        "s#mmPOnsmO-P",
        "F-000B0C?0-0",
    ),
    (
        "CTUN",
        "Qffffffffffff",
        "TimeUS,ThI,ABst,ThO,ThH,DAlt,Alt,BAlt,DSAlt,SAlt,TAlt,DCRt,CRt",
        "s----mmmmmmnn",
        "F----00000000",
    ),
    (
        "RCOU",
        "QHHHHHHHHHHHHHH",
        "TimeUS,C1,C2,C3,C4,C5,C6,C7,C8,C9,C10,C11,C12,C13,C14",
        "sYYYYYYYYYYYYYY",
        "F--------------",
    ),
    (
        "BAT",
        "QBfffffcfBBB",
        "TimeUS,Inst,Volt,VoltR,Curr,CurrTot,EnrgTot,Temp,Res,RemPct,H,SH",
        "s#vvAaXOw%-%",
        "F-000C0?0000",
    ),
    (
        "VIBE",
        "QBfffI",
        "TimeUS,IMU,VibeX,VibeY,VibeZ,Clip",
        "s#ooo-",
        "F-000-",
    ),
];

/// The PID loops' messages, which only a flight with PID logging on
/// writes, defined after the types above; the four share a format.
const PID_TYPES: [&str; 4] = ["PIDR", "PIDP", "PIDY", "PIDA"];
const PID_FORMAT: &str = "QffffffffffB";
const PID_LABELS: &str = "TimeUS,Tar,Act,Err,P,I,D,FF,DFF,Dmod,SRate,Flags";
const PID_UNITS: &str = "s-----------";
const PID_MULTIPLIERS: &str = "F-----------";

/// The unit ids the types above use, named as ArduPilot names them.
const UNITS: [(char, &str); 24] = [
    ('-', ""),
    ('?', "UNKNOWN"),
    ('#', "instance"),
    ('%', "%"),
    ('A', "A"),
    ('D', "deglatitude"),
    ('E', "rad/s"),
    ('O', "degC"),
    ('P', "Pa"),
    ('S', "satellites"),
    ('U', "deglongitude"),
    ('X', "W.h"),
    ('Y', "us"),
    ('a', "Ah"),
    ('d', "deg"),
    ('h', "degheading"),
    ('k', "deg/s"),
    ('m', "m"),
    ('n', "m/s"),
    ('o', "m/s/s"),
    ('s', "s"),
    ('v', "V"),
    ('w', "Ohm"),
    ('z', "Hz"),
];

/// The multiplier ids the types above use; the factors pass through a
/// float32 on the way into a real log, so they do here.
const MULTIPLIERS: [(char, f32); 7] = [
    ('-', 0.0),
    ('?', 1.0),
    ('0', 1.0),
    ('B', 0.01),
    ('C', 0.001),
    ('F', 1e-6),
    ('G', 1e-7),
];

/// `LOG_BITMASK`'s bit for the PID loops, `MASK_LOG_PID`.
const MASK_LOG_PID: u32 = 1 << 12;
/// The logging bitmask with the PID loops off: 4.7.0's default less
/// `MASK_LOG_PID`.
const LOG_BITMASK: u32 = 176_126;
const _: () = assert!(LOG_BITMASK | MASK_LOG_PID == 180_222);
/// `LOG_BITMASK`'s bits for the attitude at the loop rate and at 10 Hz,
/// `MASK_LOG_ATTITUDE_FAST` and `MASK_LOG_ATTITUDE_MED`. With the second
/// alone, as here, 4.7.0 logs `ANG` and `RATE` at 10 Hz, and the PID
/// loops where `MASK_LOG_PID` asks for them, beside `ATT`, which it logs
/// at 10 Hz whatever the bitmask.
const MASK_LOG_ATTITUDE_FAST: u32 = 1 << 0;
const MASK_LOG_ATTITUDE_MED: u32 = 1 << 1;
const _: () = assert!(
    LOG_BITMASK & (MASK_LOG_ATTITUDE_FAST | MASK_LOG_ATTITUDE_MED) == MASK_LOG_ATTITUDE_MED
);

/// The parameter dump: name, value, default, under 4.7.0's names, in
/// its units and with its defaults; NaN where the firmware logs none.
/// Every value invented, and those the script flies by are its own
/// constants. The off-default ones are the trims and offsets a
/// calibration leaves, the battery's setup, the frame, the flight mode
/// switch, the hover throttle the copter learned, the mission's length,
/// the survey speed, the waypoint radius and the accelerations the
/// script flies, its turn toward home in RTL and its descent with no
/// loiter over home, and the logging, from boot and with the PID loops
/// turned off.
const PARAMS: [(&str, f64, f64); 107] = [
    ("ACRO_RP_RATE", 360.0, 360.0),
    ("ACRO_Y_RATE", 202.5, 202.5),
    ("AHRS_EKF_TYPE", 3.0, 3.0),
    ("AHRS_ORIENTATION", 0.0, 0.0),
    ("AHRS_TRIM_X", 0.0031, 0.0),
    ("AHRS_TRIM_Y", -0.0018, 0.0),
    ("ARMING_SKIPCHK", 0.0, 0.0),
    ("ATC_ACC_P_MAX", 1100.0, 1100.0),
    ("ATC_ACC_R_MAX", 1100.0, 1100.0),
    ("ATC_ACC_Y_MAX", 270.0, 270.0),
    ("ATC_ANGLE_MAX", 30.0, 30.0),
    ("ATC_ANG_PIT_P", 4.5, 4.5),
    ("ATC_ANG_RLL_P", 4.5, 4.5),
    ("ATC_ANG_YAW_P", 4.5, 4.5),
    ("ATC_RAT_PIT_P", 0.135, 0.135),
    ("ATC_RAT_PIT_I", 0.135, 0.135),
    ("ATC_RAT_PIT_D", 0.0036, 0.0036),
    ("ATC_RAT_RLL_P", 0.135, 0.135),
    ("ATC_RAT_RLL_I", 0.135, 0.135),
    ("ATC_RAT_RLL_D", 0.0036, 0.0036),
    ("ATC_RAT_YAW_P", 0.18, 0.18),
    ("ATC_RAT_YAW_I", 0.018, 0.018),
    ("ATC_THR_MIX_MAN", 0.1, 0.1),
    ("BARO1_GND_PRESS", 97_682.0, f64::NAN),
    ("BATT_MONITOR", 4.0, 0.0),
    ("BATT_CAPACITY", CAPACITY, 3300.0),
    ("BATT_LOW_VOLT", 14.0, 10.5),
    ("BATT_FS_LOW_ACT", 2.0, 0.0),
    ("BRD_SAFETYOPTION", 3.0, 3.0),
    ("COMPASS_DEC", 0.0, 0.0),
    ("COMPASS_OFS_X", -12.4, 0.0),
    ("COMPASS_OFS_Y", 8.7, 0.0),
    ("COMPASS_OFS_Z", -21.1, 0.0),
    ("COMPASS_USE", 1.0, 1.0),
    ("EK3_ENABLE", 1.0, 1.0),
    ("EK3_SRC1_POSXY", 3.0, 3.0),
    ("EK3_SRC1_VELXY", 3.0, 3.0),
    ("EK3_SRC1_POSZ", 1.0, 1.0),
    ("EK3_SRC1_YAW", 1.0, 1.0),
    ("FENCE_ENABLE", 0.0, 0.0),
    ("FENCE_ALT_MAX", 100.0, 100.0),
    ("FENCE_RADIUS", 300.0, 300.0),
    ("FLTMODE1", 0.0, 0.0),
    ("FLTMODE2", 2.0, 0.0),
    ("FLTMODE3", 5.0, 0.0),
    ("FLTMODE4", 3.0, 0.0),
    ("FLTMODE5", 6.0, 0.0),
    ("FLTMODE6", 9.0, 0.0),
    ("FRAME_CLASS", 1.0, 0.0),
    ("FRAME_TYPE", 1.0, 1.0),
    ("FS_GCS_ENABLE", 0.0, 0.0),
    ("FS_THR_ENABLE", 1.0, 1.0),
    ("FS_THR_VALUE", 975.0, 975.0),
    ("GPS1_TYPE", 1.0, 1.0),
    ("GPS_HDOP_GOOD", 140.0, 140.0),
    ("INS_GYRO_FILTER", 20.0, 20.0),
    ("INS_ACCEL_FILTER", 20.0, 20.0),
    ("INS_USE", 1.0, 1.0),
    ("INS_USE2", 1.0, 1.0),
    ("LAND_SPD_MS", LAND_SLOW, 0.5),
    ("LAND_SPD_HIGH_MS", 0.0, 0.0),
    // the flight's: `Variant::param`; 4.7.0's default logs the PID loops
    ("LOG_BITMASK", 176_126.0, 180_222.0),
    // the log runs from boot
    ("LOG_DISARMED", 1.0, 0.0),
    ("LOG_FILE_BUFSIZE", 200.0, 200.0),
    ("MAV_SYSID", 1.0, 1.0),
    // this value and those of MOT_THST_HOVER and WP_SPD stand for the
    // flight's own, which `Variant::param` gives in their place
    ("MIS_TOTAL", 13.0, 0.0),
    ("MOT_PWM_MIN", 1000.0, 1000.0),
    ("MOT_PWM_MAX", 2000.0, 2000.0),
    ("MOT_SPIN_ARM", SPIN_ARM, 0.1),
    ("MOT_SPIN_MIN", 0.15, 0.15),
    ("MOT_SPIN_MAX", 0.95, 0.95),
    ("MOT_THST_EXPO", 0.65, 0.65),
    // the flight's: `Variant::param`
    ("MOT_THST_HOVER", HOVER, 0.35),
    ("MOT_HOVER_LEARN", 2.0, 2.0),
    ("PILOT_SPD_UP", CLIMB_UP, 2.5),
    ("PILOT_SPD_DN", 0.0, 0.0),
    ("PILOT_ACC_Z", VZ_ACCEL, 2.5),
    ("PILOT_THR_BHV", 0.0, 0.0),
    ("PSC_D_ACC_P", 0.05, 0.05),
    ("PSC_D_ACC_I", 0.1, 0.1),
    ("PSC_D_POS_P", 1.0, 1.0),
    ("PSC_D_VEL_P", 5.0, 5.0),
    ("PSC_NE_POS_P", 1.0, 1.0),
    ("PSC_NE_VEL_P", 2.0, 2.0),
    ("PSC_NE_VEL_I", 1.0, 1.0),
    ("RC_SPEED", 490.0, 490.0),
    ("RCMAP_ROLL", 1.0, 1.0),
    ("RCMAP_PITCH", 2.0, 2.0),
    ("RCMAP_THROTTLE", 3.0, 3.0),
    ("RCMAP_YAW", 4.0, 4.0),
    ("RTL_ALT_M", 15.0, 15.0),
    ("RTL_ALT_FINAL_M", 0.0, 0.0),
    ("RTL_LOIT_TIME", 0.0, 5000.0),
    ("RTL_SPEED_MS", 0.0, 0.0),
    ("SCHED_LOOP_RATE", LOOP_RATE, 400.0),
    ("SERVO1_FUNCTION", 33.0, 33.0),
    ("SERVO2_FUNCTION", 34.0, 34.0),
    ("SERVO3_FUNCTION", 35.0, 35.0),
    ("SERVO4_FUNCTION", 36.0, 36.0),
    ("THR_DZ", 100.0, 100.0),
    ("WP_ACC", ACCEL, 2.5),
    ("WP_ACC_Z", VZ_ACCEL, 1.0),
    ("WP_RADIUS_M", WP_RADIUS, 2.0),
    // the flight's: `Variant::param`
    ("WP_SPD", 8.0, 10.0),
    ("WP_SPD_DN", CLIMB_DOWN, 1.5),
    ("WP_SPD_UP", CLIMB_UP, 2.5),
    // facing the next waypoint, home in RTL included
    ("WP_YAW_BEHAVIOR", 1.0, 2.0),
];

/// Which of the demo's flights to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flight {
    /// The demo flight.
    A,
    /// The second sortie, which the demo pair compares with the first.
    B,
}

impl Flight {
    /// What the opened log is called.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Flight::A => NAME,
            Flight::B => NAME_SECOND,
        }
    }

    fn variant(self) -> &'static Variant {
        match self {
            Flight::A => &VARIANT_A,
            Flight::B => &VARIANT_B,
        }
    }
}

/// What sets the two flights apart, the knobs the script reads. Flight
/// A's are the script's own constants, so a knob added for B leaves A's
/// arithmetic, and its log, as they were.
struct Variant {
    /// The milliseconds into the GPS week at boot.
    gps_ms_at_boot: u64,
    /// Arming, the takeoff and Auto, in ticks; the script from arming on
    /// runs from them.
    arm: u64,
    takeoff: u64,
    auto: u64,
    /// The ground station's change of the survey speed, and the GPS
    /// fault, each where the flight has one.
    speed_change: Option<u64>,
    fault: Option<u64>,
    /// Meters north of home the survey's first leg lies; with `transit`,
    /// the mission flies there first, as a waypoint of its own.
    north: f64,
    transit: bool,
    /// The speed over the legs from the start, meters per second.
    cruise: f64,
    /// The west wind's lean and its two gusts', meters per second squared.
    wind: [f64; 3],
    /// The throttle that hovers.
    hover: f64,
    /// The PID loops are logged, and the dump's `LOG_BITMASK` has the
    /// PID bit set: see [`Variant::param`].
    pid_logging: bool,
    /// Values of the parameter dump set apart from A's, besides those
    /// the knobs above set; see [`Variant::param`].
    params: &'static [(&'static str, f64)],
    /// The noise generator's seed.
    seed: u64,
}

impl Variant {
    /// The mission's waypoints, home included.
    fn waypoints(&self) -> usize {
        WAYPOINTS + usize::from(self.transit)
    }

    /// The dump's value for `name` where the flight sets it: the
    /// mission's items, home, the waypoints and the RTL after them, its
    /// speed, its hover throttle and the logging bitmask, as the flight
    /// flies and logs them; else where `params` sets it apart from A's.
    fn param(&self, name: &str) -> Option<f64> {
        match name {
            "MIS_TOTAL" => return Some((self.waypoints() + 1) as f64),
            "WP_SPD" => return Some(self.cruise),
            "MOT_THST_HOVER" => return Some(self.hover),
            "LOG_BITMASK" => {
                let pids = if self.pid_logging { MASK_LOG_PID } else { 0 };
                return Some(f64::from(LOG_BITMASK | pids));
            }
            _ => {}
        }
        self.params
            .iter()
            .find_map(|&(n, value)| (n == name).then_some(value))
    }

    /// The value the flight's dump logs for `name`, one of [`PARAMS`].
    fn dump(&self, name: &str) -> f64 {
        self.param(name).unwrap_or_else(|| {
            PARAMS
                .iter()
                .find_map(|&(n, value, _)| (n == name).then_some(value))
                .expect("a parameter of the dump")
        })
    }
}

/// The demo flight.
const VARIANT_A: Variant = Variant {
    gps_ms_at_boot: GPS_MS_AT_BOOT,
    arm: T_ARM,
    takeoff: T_TAKEOFF,
    auto: T_AUTO,
    speed_change: Some(T_SPEED),
    fault: Some(T_FAULT),
    north: 0.0,
    transit: false,
    cruise: 8.0,
    wind: [0.8, 0.25, 0.15],
    hover: HOVER,
    pid_logging: false,
    params: &[],
    seed: 0x9E37_79B9_7F4A_7C15,
};

/// Flight B's script from arming on runs this much sooner than A's.
const B_SOONER: u64 = ticks(12.0);
/// Flight B's hover throttle, learned since A.
const B_HOVER: f64 = 0.44;

/// The second sortie.
const VARIANT_B: Variant = Variant {
    // two hours after A's boot
    gps_ms_at_boot: GPS_MS_AT_BOOT + 7_200_000,
    arm: T_ARM - B_SOONER,
    takeoff: T_TAKEOFF - B_SOONER,
    auto: T_AUTO - B_SOONER,
    speed_change: None,
    fault: None,
    north: 250.0,
    transit: true,
    cruise: 10.0,
    wind: [1.4, 0.35, 0.25],
    hover: B_HOVER,
    pid_logging: true,
    params: &[
        ("ATC_RAT_PIT_P", 0.15),
        ("ATC_RAT_RLL_P", 0.15),
        ("BARO1_GND_PRESS", 97_655.0),
    ],
    seed: 0xD1B5_4A32_D192_ED03,
};

/// One of the demo's flights as the bytes of a log.
#[must_use]
pub fn bytes(flight: Flight) -> Vec<u8> {
    let mut demo = Demo::new(flight.variant());
    demo.run();
    demo.w.into_bytes()
}

/// A time in the script as a tick.
const fn ticks(seconds: f64) -> u64 {
    (seconds * 100.0 + 0.5) as u64
}

/// Waypoint `index` of the flight's mission, meters north and east of
/// home: the legs run east and back, each 40 m north of the last, the
/// first `v.north` of home, and with `v.transit` the mission flies to its
/// start first. Index 0 is home, where the mission starts; it is not
/// flown to.
fn waypoint(index: usize, v: &Variant) -> [f64; 2] {
    let Some(index) = index.checked_sub(usize::from(v.transit)) else {
        return [0.0, 0.0];
    };
    let leg = (index / 2) as f64;
    let east = if matches!(index % 4, 1 | 2) { LEG } else { 0.0 };
    [v.north + leg * SPACING, east]
}

/// A PID loop's gains and its integrator's limit.
#[derive(Clone, Copy)]
struct Gains {
    p: f64,
    i: f64,
    d: f64,
    imax: f64,
}

/// The limit of the rate loops' integrators and the yaw rate's D gain,
/// which the dump does not log: 4.7.0's defaults.
const RATE_IMAX: f64 = 0.5;
const RATE_YAW_D: f64 = 0.0;
/// The vertical acceleration loop's D gain and integrator limit, which
/// the dump does not log: 4.7.0's defaults for a copter. The loop works
/// in m/s/s, down-positive.
const ACCEL_D: f64 = 0.0;
const ACCEL_IMAX: f64 = 0.8;
/// Seconds, the lag of the flown rate behind the one asked for.
const LOOP_LAG: f64 = 0.03;
/// Seconds, the lag of the filter on the throttle's slew rate.
const SLEW_LAG: f64 = 0.05;

/// How a PID loop's integrator runs, in the air and on the ground.
#[derive(Clone, Copy)]
enum Integrator {
    /// A rate loop's: the error integrated in the air, within the limit,
    /// and zero on the ground, as Copter resets the rate loops' I terms
    /// while landed.
    Integrated,
    /// The vertical acceleration loop's. 4.7.0 sends the motors the
    /// hover throttle less the loop's output (`D_update_controller`), so
    /// in the air its integrator holds what the throttle asks beyond the
    /// hover throttle, P and D, as `D_init_controller` sets it; on the
    /// ground, where the landed modes relax it with a zero throttle
    /// (`D_relax_controller(0)`), the hover throttle.
    Throttle { hover: f64 },
}

/// One PID loop as `PIDR`, `PIDP`, `PIDY` or `PIDA` log it, and `RATE`
/// its target, what was flown and its output, stepped with the
/// simulation so its lag and integrator run at 100 Hz whatever rate it
/// is logged at; a record writes it as it stands, so `Err` is always
/// `Tar` minus `Act` and `P` the gain times `Err`.
#[derive(Clone, Copy)]
struct PidLoop {
    gains: Gains,
    integrates: Integrator,
    /// What the loop is asked for and what was flown, noise included.
    target: f64,
    actual: f64,
    /// What was flown, before the noise: the target, lagged.
    flown: f64,
    /// The error of the last step on which the copter flew, which the D
    /// term differences; zero on the ground.
    last_error: f64,
    integrator: f64,
    derivative: f64,
}

impl PidLoop {
    fn new(gains: Gains, integrates: Integrator) -> Self {
        Self {
            gains,
            integrates,
            target: 0.0,
            actual: 0.0,
            flown: 0.0,
            last_error: 0.0,
            integrator: 0.0,
            derivative: 0.0,
        }
    }

    fn error(&self) -> f64 {
        self.target - self.actual
    }

    fn p_term(&self) -> f64 {
        self.gains.p * self.error()
    }

    fn d_term(&self) -> f64 {
        self.gains.d * self.derivative
    }

    /// What the loop sends on: P, I and D, with no feed-forward.
    fn output(&self) -> f64 {
        self.p_term() + self.integrator + self.d_term()
    }

    /// One step toward `target`, `noise` on what is flown, `throttle_in`
    /// the throttle the climb asks before the lean's boost. On the
    /// ground the last error is held at zero, so the D term starts afresh
    /// at the takeoff; the integrator runs as `integrates` says.
    fn step(&mut self, target: f64, noise: f64, flying: bool, throttle_in: f64) {
        self.target = target;
        self.flown += (target - self.flown) * (DT / LOOP_LAG);
        self.actual = self.flown + noise;
        let error = self.error();
        if flying {
            self.derivative = (error - self.last_error) / DT;
            self.last_error = error;
        } else {
            self.derivative = 0.0;
            self.last_error = 0.0;
        }
        let Gains { p, i, d, imax } = self.gains;
        self.integrator = match (self.integrates, flying) {
            (Integrator::Integrated, true) => (self.integrator + i * error * DT).clamp(-imax, imax),
            (Integrator::Integrated, false) => 0.0,
            (Integrator::Throttle { hover }, true) => {
                (hover - throttle_in - p * error - d * self.derivative).clamp(-imax, imax)
            }
            (Integrator::Throttle { hover }, false) => hover,
        };
    }
}

/// The flight's PID loops: the rates', roll, pitch and yaw, on its dump's
/// gains, and the vertical acceleration's.
fn pid_loops_of(v: &Variant) -> [PidLoop; 4] {
    let rate = |axis: &str, d: f64| {
        let gains = Gains {
            p: v.dump(&format!("ATC_RAT_{axis}_P")),
            i: v.dump(&format!("ATC_RAT_{axis}_I")),
            d,
            imax: RATE_IMAX,
        };
        PidLoop::new(gains, Integrator::Integrated)
    };
    let accel = Gains {
        p: v.dump("PSC_D_ACC_P"),
        i: v.dump("PSC_D_ACC_I"),
        d: ACCEL_D,
        imax: ACCEL_IMAX,
    };
    [
        rate("RLL", v.dump("ATC_RAT_RLL_D")),
        rate("PIT", v.dump("ATC_RAT_PIT_D")),
        rate("YAW", RATE_YAW_D),
        PidLoop::new(accel, Integrator::Throttle { hover: v.hover }),
    ]
}

/// Where the flight is, for what the step does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// On the field, before the takeoff.
    Ground,
    /// Climbing to the survey's height over home.
    Takeoff,
    /// The mission's waypoints.
    Survey,
    /// Home at the survey's height.
    Return,
    /// Over home, coming down until the pilot takes over.
    Descent,
    /// Down onto the field.
    Landing,
    /// Back on the field.
    Landed,
}

/// The simulated copter, with the writer it logs to.
struct Demo {
    w: LogWriter,
    /// The flight it flies.
    v: &'static Variant,
    noise: Noise,
    tick: u64,
    phase: Phase,
    /// Meters from home: north, east, up.
    pos: [f64; 3],
    vel: [f64; 3],
    /// The horizontal acceleration of the last step, north and east,
    /// which the attitude leans for.
    acc: [f64; 2],
    /// The vertical acceleration and the climb rate asked for, of the
    /// last step.
    vz_acc: f64,
    vz_want: f64,
    /// The next waypoint of the survey.
    wp: usize,
    /// The speed over the legs, meters per second: `WP_SPD`.
    cruise: f64,
    target_alt: f64,
    armed: bool,
    /// The copter has left the ground once.
    airborne: bool,
    /// Roll, pitch and yaw, degrees, and the heading the yaw turns to.
    att: [f64; 3],
    yaw_target: f64,
    /// The body rates of the last step, radians per second.
    rates: [f64; 3],
    /// The throttle out, 0 to 1, its rate of change per second,
    /// filtered, and the current it draws, amperes.
    throttle: f64,
    throttle_slew: f64,
    current: f64,
    /// The charge drawn, mAh, and the energy, Wh.
    used_mah: f64,
    used_wh: f64,
    /// The receiver reports a fault.
    fault: bool,
    landed_at: Option<u64>,
    /// The `ID` of the last `MSG`: 4.7.0 numbers its messages from 1,
    /// never 0 (`get_MSG_id`), and logs each of these in one chunk,
    /// `Seq` 0.
    msg_id: u8,
    /// The PID loops, roll, pitch, yaw and the vertical acceleration,
    /// which `RATE` logs and, where the flight logs them, the PID
    /// messages; and the noise they draw: a generator of their own, so
    /// the flight's other records keep their values.
    pids: [PidLoop; 4],
    pid_noise: Noise,
}

impl Demo {
    fn new(v: &'static Variant) -> Self {
        Self {
            w: LogWriter::new(),
            v,
            noise: Noise(v.seed),
            tick: 0,
            phase: Phase::Ground,
            pos: [0.0; 3],
            vel: [0.0; 3],
            acc: [0.0; 2],
            vz_acc: 0.0,
            vz_want: 0.0,
            wp: 0,
            cruise: v.cruise,
            target_alt: 0.0,
            armed: false,
            airborne: false,
            // parked facing east, down the first leg
            att: [0.0, 0.0, 90.0],
            yaw_target: 90.0,
            rates: [0.0; 3],
            throttle: 0.0,
            throttle_slew: 0.0,
            current: 0.0,
            used_mah: 0.0,
            used_wh: 0.0,
            fault: false,
            landed_at: None,
            msg_id: 0,
            pids: pid_loops_of(v),
            pid_noise: Noise(0xA076_1D64_78BD_642F),
        }
    }

    /// The whole log: the definitions, then the script tick by tick.
    fn run(&mut self) {
        self.define();
        while self.tick < MAX_TICKS {
            self.scheduled();
            self.step();
            self.streams();
            if self
                .landed_at
                .is_some_and(|landed| self.tick >= landed + AFTER_LAND_END)
            {
                break;
            }
            self.tick += 1;
        }
    }

    fn secs(&self) -> f64 {
        self.tick as f64 * DT
    }

    /// The time field of a record written this tick, `offset`
    /// microseconds into it.
    fn us(&self, offset: u64) -> Value {
        Value::U64(self.tick * TICK_US + offset)
    }

    /// Write one record. The definitions and the records are fixed
    /// together here, so a mismatch is a bug in this file.
    fn record(&mut self, name: &str, values: &[Value]) {
        self.w
            .record(name, values)
            .expect("the demo's records match its definitions");
    }

    fn define(&mut self) {
        for (id, (name, format, labels, _, _)) in (1u8..).zip(TYPES) {
            let labels: Vec<&str> = labels.split(',').collect();
            self.w
                .define(id, name, format, &labels)
                .expect("the demo's definitions are valid");
        }
        if self.v.pid_logging {
            let labels: Vec<&str> = PID_LABELS.split(',').collect();
            for (id, name) in (Self::FIRST_PID_ID..).zip(PID_TYPES) {
                self.w
                    .define(id, name, PID_FORMAT, &labels)
                    .expect("the demo's definitions are valid");
            }
        }
    }

    /// The id of the first PID type, after [`TYPES`].
    const FIRST_PID_ID: u8 = TYPES.len() as u8 + 1;

    /// The units, multipliers and their assignment per type, as a log's
    /// first records.
    fn tables(&mut self) {
        let t = self.us(0);
        for (id, label) in UNITS {
            self.record(
                "UNIT",
                &[
                    t.clone(),
                    Value::I64(i64::from(id as u8)),
                    Value::Str(label.into()),
                ],
            );
        }
        for (id, factor) in MULTIPLIERS {
            self.record(
                "MULT",
                &[
                    t.clone(),
                    Value::I64(i64::from(id as u8)),
                    Value::F64(f64::from(factor)),
                ],
            );
        }
        let pid_types = if self.v.pid_logging {
            PID_TYPES.len()
        } else {
            0
        };
        let tables = TYPES
            .iter()
            .map(|&(_, _, _, units, multipliers)| (units, multipliers))
            .chain(std::iter::repeat_n((PID_UNITS, PID_MULTIPLIERS), pid_types));
        for (id, (units, multipliers)) in (1u64..).zip(tables) {
            self.record(
                "FMTU",
                &[
                    t.clone(),
                    Value::U64(id),
                    Value::Str(units.into()),
                    Value::Str(multipliers.into()),
                ],
            );
        }
    }

    fn msg(&mut self, text: &str, offset: u64) {
        self.msg_id = self.msg_id.wrapping_add(1);
        if self.msg_id == 0 {
            self.msg_id = 1;
        }
        let id = u64::from(self.msg_id);
        self.record(
            "MSG",
            &[
                self.us(offset),
                Value::U64(id),
                Value::U64(0),
                Value::Str(text.into()),
            ],
        );
    }

    fn param(&mut self, name: &str, value: f64, default: f64, offset: u64) {
        self.record(
            "PARM",
            &[
                self.us(offset),
                Value::Str(name.into()),
                Value::F64(value),
                Value::F64(default),
            ],
        );
    }

    fn mode(&mut self, number: u64, reason: u64, offset: u64) {
        self.record(
            "MODE",
            &[
                self.us(offset),
                Value::U64(number),
                Value::U64(number),
                Value::U64(reason),
            ],
        );
    }

    fn event(&mut self, id: u64, offset: u64) {
        self.record("EV", &[self.us(offset), Value::U64(id)]);
    }

    fn error(&mut self, subsystem: u64, code: u64, offset: u64) {
        self.record(
            "ERR",
            &[self.us(offset), Value::U64(subsystem), Value::U64(code)],
        );
    }

    /// What the script says happens at this tick, and what follows the
    /// landing.
    fn scheduled(&mut self) {
        let v = self.v;
        match self.tick {
            0 => {
                self.tables();
                self.msg("ArduCopter V4.7.0 (0000000)", 100);
                self.msg("Frame: QUAD/X", 200);
                self.msg("RCOut: PWM:1-14", 300);
            }
            T_PARAMS..T_PARAMS_END => {
                let (name, value, default) = PARAMS[(self.tick - T_PARAMS) as usize];
                self.param(name, v.param(name).unwrap_or(value), default, 0);
            }
            T_BOOT_MODE => self.mode(STABILIZE, REASON_INITIALIZED, 0),
            T_EKF_GPS => self.msg("EKF3 IMU0 is using GPS", 0),
            t if t == T_EKF_GPS + 10 => self.msg("EKF3 IMU1 is using GPS", 0),
            T_ORIGIN => self.msg("EKF3 IMU0 origin set", 0),
            t if t == T_ORIGIN + 5 => self.event(EV_HOME_SET, 0),
            t if t == v.arm => self.mode(LOITER, REASON_RC, 0),
            t if t == v.arm + 20 => self.msg("Arming motors", 0),
            t if t == v.arm + 25 => {
                self.event(EV_ARMED, 0);
                self.armed = true;
            }
            t if t == v.takeoff => {
                self.phase = Phase::Takeoff;
                self.target_alt = SURVEY_ALT;
            }
            t if t == v.auto => {
                self.mode(AUTO, REASON_RC, 0);
                self.event(EV_AUTO_ARMED, 100);
                // item 0 is home, where the copter is
                self.msg("Mission: 1 WP", 200);
                self.phase = Phase::Survey;
                self.wp = 1;
            }
            // a set from the ground station is logged twice
            t if v.speed_change.is_some_and(|at| t == at || t == at + 5) => {
                self.param("WP_SPD", 10.0, 10.0, 0);
                self.cruise = 10.0;
            }
            t if v.fault == Some(t) => {
                self.error(GPS_SUBSYSTEM, CODE_UNHEALTHY, 0);
                self.event(EV_LOST_GPS, 100);
                self.fault = true;
            }
            t if v.fault.is_some_and(|at| t == at + FAULT_TICKS) => {
                self.error(GPS_SUBSYSTEM, CODE_RESOLVED, 0);
                self.fault = false;
            }
            _ => {}
        }
        if let Some(landed) = self.landed_at {
            match self.tick - landed {
                AFTER_LAND_EVENT => self.event(EV_LAND_COMPLETE, 0),
                AFTER_LAND_DISARM => {
                    self.event(EV_DISARMED, 0);
                    self.msg("Disarming motors", 100);
                    self.armed = false;
                }
                AFTER_LAND_MODE => self.mode(STABILIZE, REASON_RC, 0),
                _ => {}
            }
        }
    }

    /// One step of the flight: where the copter goes, then how it leans
    /// and what it draws.
    fn step(&mut self) {
        match self.phase {
            Phase::Ground | Phase::Landed => {}
            Phase::Takeoff => {
                self.fly_toward([0.0, 0.0]);
                self.climb_to(self.target_alt, CLIMB_UP, CLIMB_DOWN);
                if self.pos[2] > 0.5 && !self.airborne {
                    self.airborne = true;
                    self.event(EV_NOT_LANDED, 300);
                }
            }
            Phase::Survey | Phase::Return => {
                let target = if self.phase == Phase::Survey {
                    waypoint(self.wp, self.v)
                } else {
                    [0.0, 0.0]
                };
                let distance = self.fly_toward(target);
                if distance > 5.0 {
                    self.yaw_target = course(self.pos, target);
                }
                self.climb_to(self.target_alt, CLIMB_UP, CLIMB_DOWN);
                if distance < WP_RADIUS {
                    self.arrive();
                }
            }
            Phase::Descent => {
                self.fly_toward([0.0, 0.0]);
                self.climb_to(0.0, CLIMB_UP, CLIMB_DOWN);
                if self.pos[2] <= LAND_SWITCH_ALT {
                    self.mode(LAND, REASON_RC, 300);
                    self.phase = Phase::Landing;
                }
            }
            Phase::Landing => {
                self.fly_toward([0.0, 0.0]);
                let rate = if self.pos[2] > 10.0 {
                    LAND_FAST
                } else {
                    LAND_SLOW
                };
                // aimed below the field, so the descent keeps its rate
                // down to the ground rather than easing off above it
                self.climb_to(-1.0, CLIMB_UP, rate);
                if self.pos[2] <= 0.0 {
                    self.pos[2] = 0.0;
                    self.vel = [0.0; 3];
                    self.acc = [0.0; 2];
                    self.vz_acc = 0.0;
                    self.vz_want = 0.0;
                    self.target_alt = 0.0;
                    self.phase = Phase::Landed;
                    self.landed_at = Some(self.tick);
                }
            }
        }
        self.attitude();
        self.step_pid_loops();
        self.power();
    }

    /// The PID loops one step on: the body rates the attitude turned at,
    /// in rad/s, and the vertical acceleration, in m/s/s and
    /// down-positive, as 4.7.0's loops take them, with the throttle the
    /// climb asks of the vertical one.
    fn step_pid_loops(&mut self) {
        let flying = self.flying();
        let throttle_in = self.throttle_in();
        let targets = [self.rates[0], self.rates[1], self.rates[2], -self.vz_acc];
        let n = &mut self.pid_noise;
        for (pid, (target, spread)) in self
            .pids
            .iter_mut()
            .zip(targets.into_iter().zip([0.01, 0.01, 0.008, 0.05]))
        {
            pid.step(target, n.gauss() * spread, flying, throttle_in);
        }
    }

    /// At the survey's next waypoint, on to the one after it or home; at
    /// home, down. The records of the tick's script come first, so what
    /// is logged here comes after them.
    fn arrive(&mut self) {
        if self.phase == Phase::Return {
            self.phase = Phase::Descent;
            return;
        }
        self.wp += 1;
        // the mission's items: home, the waypoints, then RTL
        let waypoints = self.v.waypoints();
        if self.wp < waypoints {
            self.msg(&format!("Mission: {} WP", self.wp), 300);
        } else {
            self.msg(&format!("Mission: {waypoints} RTL"), 300);
            self.mode(RTL, REASON_MISSION_END, 400);
            self.phase = Phase::Return;
        }
    }

    /// Fly toward `target`, meters north and east, at the cruise speed,
    /// slowing to stop there; returns the distance left before the step.
    fn fly_toward(&mut self, target: [f64; 2]) -> f64 {
        let dn = target[0] - self.pos[0];
        let de = target[1] - self.pos[1];
        let distance = (dn * dn + de * de).sqrt();
        // the speed from which the copter can still stop at the target,
        // and settle on it
        let speed = self
            .cruise
            .min((2.0 * ACCEL * distance).sqrt())
            .min(distance / SETTLE);
        let want = if distance > 1e-6 {
            [dn / distance * speed, de / distance * speed]
        } else {
            [0.0, 0.0]
        };
        let dvn = want[0] - self.vel[0];
        let dve = want[1] - self.vel[1];
        let need = (dvn * dvn + dve * dve).sqrt();
        let cap = ACCEL * DT;
        let k = if need > cap { cap / need } else { 1.0 };
        self.acc = [dvn * k / DT, dve * k / DT];
        self.vel[0] += dvn * k;
        self.vel[1] += dve * k;
        self.pos[0] += self.vel[0] * DT;
        self.pos[1] += self.vel[1] * DT;
        distance
    }

    /// Climb or descend toward `alt` at up to the given rates, easing in
    /// over the last meters.
    fn climb_to(&mut self, alt: f64, up: f64, down: f64) {
        self.vz_want = ((alt - self.pos[2]) / 2.0).clamp(-down, up);
        let dv = (self.vz_want - self.vel[2]).clamp(-VZ_ACCEL * DT, VZ_ACCEL * DT);
        self.vz_acc = dv / DT;
        self.vel[2] += dv;
        self.pos[2] += self.vel[2] * DT;
    }

    fn flying(&self) -> bool {
        matches!(
            self.phase,
            Phase::Takeoff | Phase::Survey | Phase::Return | Phase::Descent | Phase::Landing
        )
    }

    /// The lean the acceleration and the wind ask for, followed with a
    /// lag, and the turn toward the heading, at its rate.
    fn attitude(&mut self) {
        let t = self.secs();
        // a wind from the west, gusting a little: the copter leans into it
        let [wind, slow_gust, quick_gust] = self.v.wind;
        let lean_east = if self.flying() {
            -wind - slow_gust * (t / 7.3).sin() - quick_gust * (t / 2.9).sin()
        } else {
            0.0
        };
        let a_north = self.acc[0];
        let a_east = self.acc[1] + lean_east;
        let (sin, cos) = self.att[2].to_radians().sin_cos();
        let a_forward = a_north * cos + a_east * sin;
        let a_right = -a_north * sin + a_east * cos;
        let want = [
            (a_right / G).atan().to_degrees(),
            -(a_forward / G).atan().to_degrees(),
        ];
        let mut next = self.att;
        for axis in 0..2 {
            next[axis] += (want[axis] - self.att[axis]) * (DT / ATT_LAG);
        }
        let mut diff = (self.yaw_target - self.att[2]).rem_euclid(360.0);
        if diff > 180.0 {
            diff -= 360.0;
        }
        let turn = diff.clamp(-YAW_RATE * DT, YAW_RATE * DT);
        next[2] = (self.att[2] + turn).rem_euclid(360.0);
        self.rates = [
            (next[0] - self.att[0]).to_radians() / DT,
            (next[1] - self.att[1]).to_radians() / DT,
            turn.to_radians() / DT,
        ];
        self.att = next;
    }

    /// The throttle for the thrust, its slew rate, and the battery it
    /// drains.
    fn power(&mut self) {
        let throttle = if !self.armed {
            0.0
        } else if !self.flying() {
            SPIN_ARM
        } else {
            (self.throttle_in() / self.tilt()).clamp(0.1, 0.9)
        };
        let slew = (throttle - self.throttle).abs() / DT;
        self.throttle_slew += (slew - self.throttle_slew) * (DT / SLEW_LAG);
        self.throttle = throttle;
        self.current = if !self.armed {
            0.8
        } else if !self.flying() {
            2.6
        } else {
            3.0 + 42.0 * self.throttle * self.throttle.sqrt()
        };
        // amperes over a step into mAh, and into Wh
        self.used_mah += self.current * DT / 3.6;
        self.used_wh += self.volt() * self.current * DT / 3600.0;
    }

    /// The throttle the climb asks for in the air, before the boost for
    /// the lean: the hover throttle, scaled by the vertical acceleration.
    fn throttle_in(&self) -> f64 {
        self.v.hover * (G + self.vz_acc) / G
    }

    /// The share of the thrust that holds the copter up at its lean,
    /// floored at a half.
    fn tilt(&self) -> f64 {
        (self.att[0].to_radians().cos() * self.att[1].to_radians().cos()).max(0.5)
    }

    /// The pack's voltage: 4S, sagging with the charge drawn and the
    /// current.
    fn volt(&self) -> f64 {
        16.6 - 1.4 * self.used_mah / CAPACITY - 0.04 * self.current
    }

    /// The sensor and controller records at their rates, timed after
    /// the script's, which take the step's first 500 microseconds.
    fn streams(&mut self) {
        if self.tick.is_multiple_of(4) {
            self.imu(0, 500);
            self.imu(1, 520);
        }
        if self.tick.is_multiple_of(10) {
            // in the order of 4.7.0's 10 Hz logging
            self.att_record(560);
            self.rate_record(570);
            self.pid_records(575);
            self.baro(600);
            self.ctun(640);
            self.rcou(680);
            // no position before the EKF has an origin
            if self.tick >= T_ORIGIN {
                self.pos_record(720);
            }
        }
        if self.tick.is_multiple_of(20) {
            self.gps(760);
        }
        if self.tick.is_multiple_of(100) {
            self.bat(800);
            self.vibe(840);
        }
    }

    /// The vibration the motors put into the accelerometers, and its
    /// amplitude with the throttle.
    fn vibration(&self, phase: f64) -> (f64, f64) {
        let amplitude = 0.03 + 0.3 * self.throttle;
        (
            amplitude,
            amplitude * (TAU * 6.3 * self.secs() + phase).sin(),
        )
    }

    fn imu(&mut self, instance: u64, offset: u64) {
        let (amplitude, line) = self.vibration(if instance == 0 { 0.0 } else { 1.1 });
        let jitter = 0.02 + 0.1 * self.throttle;
        let n = &mut self.noise;
        let gyro = [
            self.rates[0] + line * 0.3 + n.gauss() * 0.01,
            self.rates[1] - line * 0.2 + n.gauss() * 0.01,
            self.rates[2] + n.gauss() * 0.008,
        ];
        // the specific force is the thrust, along the body's z axis
        let thrust = {
            let [a_n, a_e] = self.acc;
            let a_up = G + self.vz_acc;
            (a_n * a_n + a_e * a_e + a_up * a_up).sqrt()
        };
        let accel = [
            line + n.gauss() * jitter,
            line * 0.7 + n.gauss() * jitter,
            -thrust + line * 1.5 + n.gauss() * jitter + amplitude * 0.1,
        ];
        let temperature = 41.5 + 0.01 * self.secs();
        self.record(
            "IMU",
            &[
                self.us(offset),
                Value::U64(instance),
                Value::F64(gyro[0]),
                Value::F64(gyro[1]),
                Value::F64(gyro[2]),
                Value::F64(accel[0]),
                Value::F64(accel[1]),
                Value::F64(accel[2]),
                Value::U64(0),
                Value::U64(0),
                Value::F64(temperature),
                Value::U64(1),
                Value::U64(1),
                Value::U64(1000),
                Value::U64(1000),
            ],
        );
    }

    /// `ATT`, and `ANG` 5 microseconds after it: 4.7.0 fills both from
    /// the same targets and estimates, `ANG` with the loop's period. It
    /// stamps `ANG` with the loop's start, a little before `ATT`; here
    /// `ANG` follows, so the file stays in time order.
    fn att_record(&mut self, offset: u64) {
        let wobble = if self.flying() {
            0.4 * (1.7 * self.secs()).sin()
        } else {
            0.0
        };
        let n = &mut self.noise;
        let roll = self.att[0] + wobble + n.gauss() * 0.15;
        let pitch = self.att[1] - wobble * 0.6 + n.gauss() * 0.15;
        let yaw = heading_field(self.att[2] + n.gauss() * 0.2);
        let period = 1.0 / LOOP_RATE + n.gauss() * 2e-5;
        let angles = [
            self.att[0],
            roll,
            self.att[1],
            pitch,
            heading_field(self.att[2]),
            yaw,
        ]
        .map(Value::F64);
        let mut att = vec![self.us(offset)];
        att.extend(angles.clone());
        att.push(Value::U64(3));
        self.record("ATT", &att);
        let mut ang = vec![self.us(offset + 5)];
        ang.extend(angles);
        ang.push(Value::F64(period));
        self.record("ANG", &ang);
    }

    /// The barometer's height above the field, drifting a little.
    fn baro_alt(&self) -> f64 {
        self.pos[2] + 0.4 * (self.secs() / 90.0).sin()
    }

    fn baro(&mut self, offset: u64) {
        let alt = self.baro_alt() + self.noise.gauss() * 0.15;
        let pressure = 101_325.0 * (1.0 - 2.255_77e-5 * (HOME_ALT + alt)).powf(5.255_88);
        let temperature = 24.5 + 0.004 * self.secs();
        let climb = self.vel[2] + self.noise.gauss() * 0.05;
        self.record(
            "BARO",
            &[
                self.us(offset),
                Value::U64(0),
                Value::F64(alt),
                Value::F64(HOME_ALT + alt),
                Value::F64(pressure),
                Value::F64(temperature),
                Value::F64(climb),
                Value::U64(self.tick * 10),
                Value::F64(0.0),
                Value::F64(24.5),
                Value::U64(1),
                // nothing to compensate for
                Value::F64(pressure),
            ],
        );
    }

    fn ctun(&mut self, offset: u64) {
        let flying = self.flying();
        let boost = if flying {
            self.throttle * (1.0 / self.tilt() - 1.0)
        } else {
            0.0
        };
        self.record(
            "CTUN",
            &[
                self.us(offset),
                Value::F64(if flying { 0.5 } else { 0.0 }),
                Value::F64(boost),
                Value::F64(self.throttle),
                Value::F64(self.v.hover),
                Value::F64(self.target_alt.max(0.0)),
                Value::F64(self.pos[2]),
                Value::F64(self.baro_alt()),
                Value::F64(0.0),
                Value::F64(0.0),
                Value::F64(0.0),
                Value::F64(self.vz_want),
                Value::F64(self.vel[2]),
            ],
        );
    }

    /// The four motors of the X frame, mixed from the body rates.
    fn rcou(&mut self, offset: u64) {
        let mut channels = [0u64; 14];
        if self.armed {
            let base = 1000.0 + 1000.0 * self.throttle;
            let [roll, pitch, yaw] = self.rates;
            let mix = [
                -roll + pitch - yaw,
                roll - pitch - yaw,
                roll + pitch + yaw,
                -roll - pitch + yaw,
            ];
            for (channel, share) in channels.iter_mut().zip(mix) {
                let pwm = base + 60.0 * share + self.noise.gauss() * 3.0;
                *channel = pwm.round().clamp(1000.0, 2000.0) as u64;
            }
        } else {
            channels[..4].fill(1000);
        }
        let mut values = vec![self.us(offset)];
        values.extend(channels.map(Value::U64));
        self.record("RCOU", &values);
    }

    /// The rate loops' targets and what was flown, in deg/s, with their
    /// outputs; the vertical acceleration asked for and flown, in m/s/s
    /// and up-positive; the throttle and its slew rate: as 4.7.0's
    /// `Write_Rate` logs them.
    fn rate_record(&mut self, offset: u64) {
        let mut values = vec![self.us(offset)];
        for pid in &self.pids[..3] {
            values.extend(
                [
                    pid.target.to_degrees(),
                    pid.actual.to_degrees(),
                    pid.output(),
                ]
                .map(Value::F64),
            );
        }
        let accel = &self.pids[3];
        values.extend(
            [
                -accel.target,
                -accel.actual,
                self.throttle,
                self.throttle_slew,
            ]
            .map(Value::F64),
        );
        self.record("RATE", &values);
    }

    /// The PID loops as they stand, where the flight logs them, 5
    /// microseconds apart from `offset` on.
    fn pid_records(&mut self, offset: u64) {
        if !self.v.pid_logging {
            return;
        }
        let pids = self.pids;
        for ((name, pid), at) in PID_TYPES.iter().zip(pids).zip((offset..).step_by(5)) {
            self.record(
                name,
                &[
                    self.us(at),
                    Value::F64(pid.target),
                    Value::F64(pid.actual),
                    Value::F64(pid.error()),
                    Value::F64(pid.p_term()),
                    Value::F64(pid.integrator),
                    Value::F64(pid.d_term()),
                    Value::F64(0.0),
                    Value::F64(0.0),
                    Value::F64(1.0),
                    Value::F64(0.0),
                    Value::U64(0),
                ],
            );
        }
    }

    /// Degrees of latitude and longitude of a point `north` and `east`
    /// meters from home.
    fn position(north: f64, east: f64) -> (f64, f64) {
        let lat = HOME_LAT + north / M_PER_DEG;
        let lon = HOME_LON + east / (M_PER_DEG * HOME_LAT.to_radians().cos());
        (lat, lon)
    }

    fn pos_record(&mut self, offset: u64) {
        let (lat, lon) = Self::position(self.pos[0], self.pos[1]);
        self.record(
            "POS",
            &[
                self.us(offset),
                Value::F64(lat),
                Value::F64(lon),
                Value::F64(HOME_ALT + self.pos[2]),
                Value::F64(self.pos[2]),
                Value::F64(self.pos[2]),
            ],
        );
    }

    /// The receiver: no fix, then a 2D fix, then 3D from 12 s, gaining
    /// satellites as it settles; the fault at 300 s costs it most of them.
    fn gps(&mut self, offset: u64) {
        let status = if self.tick < T_FIX_2D {
            1
        } else if self.tick < T_FIX_3D {
            2
        } else {
            3
        };
        let (satellites, hdop) = if self.fault {
            (5, 3.2)
        } else {
            let seen = (3 + self.tick.saturating_sub(ticks(2.0)) / ticks(2.5)).min(14);
            // one drops out now and then once the sky is in view
            let dip = u64::from(seen == 14 && (self.tick / 100) % 37 < 3);
            (seen - dip, 0.75 + 27.0 / (6.0 + self.secs()))
        };
        let fixed = status >= 3;
        let (week, week_ms) = if fixed {
            (GPS_WEEK, self.v.gps_ms_at_boot + self.tick * 10)
        } else {
            (0, 0)
        };
        let n = &mut self.noise;
        let (lat, lon) = if fixed {
            Self::position(self.pos[0] + n.gauss() * 0.4, self.pos[1] + n.gauss() * 0.4)
        } else {
            (0.0, 0.0)
        };
        let alt = if fixed {
            HOME_ALT + self.pos[2] + n.gauss() * 0.6
        } else {
            0.0
        };
        let (vn, ve) = (self.vel[0], self.vel[1]);
        let ground_speed = (vn * vn + ve * ve).sqrt();
        let course = if ground_speed > 0.5 {
            course([0.0; 3], [vn, ve])
        } else {
            0.0
        };
        let speed = if fixed {
            (ground_speed + n.gauss() * 0.05).max(0.0)
        } else {
            0.0
        };
        // the receiver's vertical velocity is positive down
        let vz = if fixed {
            -self.vel[2] + n.gauss() * 0.05
        } else {
            0.0
        };
        self.record(
            "GPS",
            &[
                self.us(offset),
                Value::U64(0),
                Value::U64(status),
                Value::U64(week_ms),
                Value::U64(week),
                Value::U64(satellites),
                Value::F64(hdop),
                Value::F64(lat),
                Value::F64(lon),
                Value::F64(alt),
                Value::F64(speed),
                Value::F64(course),
                Value::F64(vz),
                Value::F64(0.0),
                Value::U64(1),
            ],
        );
    }

    fn bat(&mut self, offset: u64) {
        let volt = self.volt() + self.noise.gauss() * 0.01;
        let resting = volt + 0.04 * self.current;
        let remaining = (100.0 - 100.0 * self.used_mah / CAPACITY).max(0.0);
        self.record(
            "BAT",
            &[
                self.us(offset),
                Value::U64(0),
                Value::F64(volt),
                Value::F64(resting),
                Value::F64(self.current),
                Value::F64(self.used_mah),
                Value::F64(self.used_wh),
                Value::F64(0.0),
                Value::F64(0.012),
                Value::U64(remaining.round() as u64),
                Value::U64(1),
                Value::U64(0),
            ],
        );
    }

    fn vibe(&mut self, offset: u64) {
        let (amplitude, _) = self.vibration(0.0);
        let n = &mut self.noise;
        let axis = |n: &mut Noise, share: f64| amplitude * share + n.gauss().abs() * 0.02;
        let x = axis(n, 0.9);
        let y = axis(n, 0.7);
        let z = axis(n, 1.3);
        self.record(
            "VIBE",
            &[
                self.us(offset),
                Value::U64(0),
                Value::F64(x),
                Value::F64(y),
                Value::F64(z),
                Value::U64(0),
            ],
        );
    }
}

/// A heading as the `f32` of a float field holds it, wrapped after the
/// rounding, so a hair under 360 degrees is logged as 0 and never as 360.
fn heading_field(degrees: f64) -> f64 {
    let rounded = f64::from(degrees.rem_euclid(360.0) as f32);
    if rounded < 360.0 { rounded } else { 0.0 }
}

/// The heading from one point to another, degrees clockwise from north.
fn course(from: [f64; 3], to: [f64; 2]) -> f64 {
    (to[1] - from[1])
        .atan2(to[0] - from[0])
        .to_degrees()
        .rem_euclid(360.0)
}

/// A seeded xorshift generator: the same noise on every run. The seed
/// must not be zero.
struct Noise(u64);

impl Noise {
    /// Uniform in 0 to 1.
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let bits = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (bits >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Bell-shaped about zero with unit spread: the sum of four uniforms.
    fn gauss(&mut self) -> f64 {
        let sum = self.uniform() + self.uniform() + self.uniform() + self.uniform();
        (sum - 2.0) * 3f64.sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EventKind, LoadedLog, SeriesKey};
    use crate::modes::Vehicle;
    use dflog::Log;

    fn loaded() -> LoadedLog {
        flight(Flight::A)
    }

    fn flight(flight: Flight) -> LoadedLog {
        LoadedLog::build(Log::from_source(bytes(flight).into()), flight.name().into())
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn the_demo_is_the_same_log_every_time() {
        for flight in [Flight::A, Flight::B] {
            let first = bytes(flight);
            let again = bytes(flight);
            assert!(first.iter().eq(again.iter()), "the bytes differ");
            // both log ANG and RATE, about 3.1 MB for A; B's PID loops
            // take it past A's bound
            let most = match flight {
                Flight::A => 3_500_000,
                Flight::B => 4_500_000,
            };
            assert!(
                (2_000_000..most).contains(&first.len()),
                "{}: {} bytes",
                flight.name(),
                first.len()
            );
        }
    }

    #[test]
    fn a_heading_is_logged_below_360() {
        let logged = |degrees: f64| heading_field(degrees).to_bits();
        assert_eq!(logged(359.999_999), 0f64.to_bits());
        assert_eq!(logged(-1e-9), 0f64.to_bits());
        assert_eq!(logged(360.0), 0f64.to_bits());
        assert_eq!(logged(-0.006), f64::from(359.994_f32).to_bits());
        assert_eq!(logged(359.99), f64::from(359.99_f32).to_bits());
        assert_eq!(logged(450.25), 90.25f64.to_bits());
    }

    #[test]
    fn the_survey_folds_back_on_itself() {
        let points: Vec<[f64; 2]> = (0..WAYPOINTS).map(|i| waypoint(i, &VARIANT_A)).collect();
        let expected = [[0.0, 0.0], [0.0, 400.0], [40.0, 400.0], [40.0, 0.0]];
        for (point, want) in points.iter().zip(expected) {
            assert!(
                close(point[0], want[0]) && close(point[1], want[1]),
                "{point:?}"
            );
        }
        let last = points[WAYPOINTS - 1];
        assert!(close(last[0], 200.0) && close(last[1], 0.0), "{last:?}");
        assert!(close(course([0.0; 3], [0.0, 400.0]), 90.0));
        assert!(close(course([40.0, 400.0, 50.0], [40.0, 0.0]), 270.0));
        assert_eq!(ticks(250.0), 25_000);

        // flight B flies out to the same survey 250 m north, then flies it
        let b: Vec<[f64; 2]> = (0..VARIANT_B.waypoints())
            .map(|i| waypoint(i, &VARIANT_B))
            .collect();
        assert_eq!(b.len(), WAYPOINTS + 1);
        assert!(close(b[0][0], 0.0) && close(b[0][1], 0.0), "home");
        for (b, a) in b[1..].iter().zip(&points) {
            assert!(close(b[0], a[0] + 250.0) && close(b[1], a[1]), "{b:?}");
        }
    }

    #[test]
    fn the_demo_reads_as_a_copter_flight() {
        let log = loaded();
        let count = |name: &str| log.type_named(name).map_or(0, |t| t.count);
        assert_eq!(log.vehicle, Vehicle::Copter);
        assert_eq!(count("PARM"), PARAMS.len() + 2);
        assert_eq!(count("MODE"), 6);
        // the streams run to the same last tick on every platform
        let duration = log.duration().unwrap();
        assert!((480.0..560.0).contains(&duration), "{duration} s");
        let per_second = |name: &str, hz: f64| count(name) as f64 / hz;
        for name in ["ATT", "ANG", "RATE", "BARO", "CTUN", "RCOU"] {
            assert!((per_second(name, 10.0) - duration).abs() < 1.0, "{name}");
        }
        assert!((per_second("IMU", 50.0) - duration).abs() < 1.0);
        assert!((per_second("GPS", 5.0) - duration).abs() < 1.0);
        assert!((per_second("BAT", 1.0) - duration).abs() < 1.5);
        assert!((per_second("VIBE", 1.0) - duration).abs() < 1.5);
        assert!((per_second("POS", 10.0) - (duration - 22.0)).abs() < 1.0);
        assert_eq!(log.type_named("IMU").unwrap().instances, [0, 1]);
        assert!(log.records() > 50_000, "{}", log.records());
        // the units the tables give the fields
        let title = |type_name: &str, field: &str| {
            log.type_named(type_name)
                .and_then(|t| t.field(field))
                .map(crate::model::Field::title)
                .unwrap()
        };
        assert_eq!(title("GPS", "HDop"), "HDop");
        assert_eq!(title("GPS", "GMS"), "GMS (s)");
        assert_eq!(title("GPS", "Spd"), "Spd (m/s)");
        assert_eq!(title("BAT", "CurrTot"), "CurrTot (Ah)");
        assert_eq!(title("BAT", "EnrgTot"), "EnrgTot (W.h)");
        assert_eq!(title("RCOU", "C1"), "C1 (us)");
        // the messages numbered from 1, each in one chunk
        let ids = field_series(&log, "MSG", "ID").ys;
        assert!(
            ids.iter()
                .enumerate()
                .all(|(i, &id)| close(id, i as f64 + 1.0))
        );
        let chunks = field_series(&log, "MSG", "Seq").ys;
        assert!(chunks.iter().all(|&seq| close(seq, 0.0)));
        assert_eq!(title("ATT", "Yaw"), "Yaw (degheading)");
        assert_eq!(title("ANG", "Dt"), "Dt (s)");
        assert_eq!(title("RATE", "RDes"), "RDes (deg/s)");
        // the file is in time order, record by record
        assert!(log.timeline.windows(2).all(|w| w[1] >= w[0]));

        // the time base, from the first 3D fix at 12 s
        let base = log.time_base.unwrap();
        assert_eq!(base.ms_offset, 12_000);

        // the track from POS, inside the survey's field: 200 m north and
        // 400 m east of home, and a little beyond in the turns
        let track = &log.track;
        assert_eq!(track.source, "POS");
        let bounds = track.bounds().unwrap();
        assert!(bounds.south >= HOME_LAT - 0.0001, "{}", bounds.south);
        assert!(bounds.north <= HOME_LAT + 0.0020, "{}", bounds.north);
        assert!(bounds.west >= HOME_LON - 0.0002, "{}", bounds.west);
        assert!(bounds.east <= HOME_LON + 0.0052, "{}", bounds.east);
        assert!(
            track
                .alts
                .iter()
                .all(|a| (HOME_ALT..HOME_ALT + 51.0).contains(a))
        );
    }

    #[test]
    fn the_demo_has_its_modes_events_and_changes() {
        let log = loaded();
        let modes: Vec<(&str, f64)> = log
            .modes
            .iter()
            .map(|m| (m.name.as_str(), m.time))
            .collect();
        let names: Vec<&str> = modes.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            ["Stabilize", "Loiter", "Auto", "RTL", "Land", "Stabilize"]
        );
        assert!((modes[0].1 - 2.2).abs() < 1e-9);
        assert!((modes[1].1 - 45.0).abs() < 1e-9);
        assert!((modes[2].1 - 80.0).abs() < 1e-9);
        let rtl = modes[3].1;
        assert!((380.0..470.0).contains(&rtl), "RTL at {rtl}");
        assert!(modes[4].1 > rtl + 20.0 && modes[5].1 > modes[4].1 + 20.0);

        let texts = |kind: EventKind| -> Vec<(f64, &str)> {
            log.events
                .iter()
                .filter(|e| e.kind == kind)
                .map(|e| ((e.time * 100.0).round() / 100.0, e.text.as_str()))
                .collect()
        };
        let lines = |kind: EventKind| -> Vec<String> {
            texts(kind)
                .iter()
                .map(|(time, text)| format!("{time:.2} {text}"))
                .collect()
        };
        assert_eq!(
            lines(EventKind::Error),
            ["300.00 GPS: unhealthy", "302.00 GPS: resolved"]
        );
        let events = texts(EventKind::Event);
        let names: Vec<&str> = events.iter().map(|(_, text)| *text).collect();
        assert_eq!(
            names,
            [
                "Home set",
                "Armed",
                "Not landed",
                "Auto armed",
                "Lost GPS",
                "Land complete",
                "Disarmed"
            ]
        );
        assert!(close(events[0].0, 22.05) && close(events[1].0, 45.25));
        assert!(close(events[3].0, 80.0) && close(events[4].0, 300.0));
        // the takeoff climbs at a meter per second squared: half a meter
        // a hundred steps after it starts, the first of them in the tick
        // that starts it
        assert!(close(events[2].0, 50.99), "{}", events[2].0);
        let landed = events[5].0;
        assert!(landed > modes[4].1 + 20.0 && close(events[6].0, landed + 1.5));
        let messages = texts(EventKind::Message);
        assert_eq!(messages[0].1, "ArduCopter V4.7.0 (0000000)");
        assert!(close(messages[0].0, 0.0));
        let mission: Vec<&str> = messages
            .iter()
            .map(|(_, text)| *text)
            .filter(|text| text.starts_with("Mission:"))
            .collect();
        // one line per item after home, each once
        assert_eq!(mission.len(), WAYPOINTS);
        assert_eq!(mission[0], "Mission: 1 WP");
        assert_eq!(mission[1], "Mission: 2 WP");
        assert_eq!(mission[WAYPOINTS - 1], "Mission: 12 RTL");
        let auto = messages
            .iter()
            .filter(|(time, text)| close(*time, 80.0) && text.starts_with("Mission:"))
            .count();
        assert_eq!(auto, 1, "one mission line as Auto starts");
        assert_eq!(lines(EventKind::Param), ["250.00 WP_SPD 8 -> 10"]);

        let speed = log.params.iter().find(|p| p.name == "WP_SPD").unwrap();
        assert!(close(speed.initial.into(), 8.0));
        assert_eq!(speed.default, Some(10.0));
        assert_eq!(speed.off_default(None), Some(false), "set to the default");
        let off_default = log
            .params
            .iter()
            .filter(|p| p.off_default(None) == Some(true))
            .count();
        assert_eq!(off_default, 24);
        // PID logging turned off, against 4.7.0's default
        let bitmask = log.params.iter().find(|p| p.name == "LOG_BITMASK").unwrap();
        assert_eq!(bitmask.off_default(None), Some(true));
        let no_default = log.params.iter().filter(|p| p.default.is_none()).count();
        assert_eq!(no_default, 1, "BARO1_GND_PRESS");
    }

    #[test]
    fn the_demo_values_are_in_range() {
        let log = loaded();
        let series = |type_name: &str, field: &str, instance: Option<i64>| {
            log.series(&SeriesKey {
                type_name: type_name.into(),
                field: field.into(),
                instance,
            })
            .unwrap()
        };
        let extent = |ys: &[f64]| {
            ys.iter()
                .fold((f64::MAX, f64::MIN), |(lo, hi), &y| (lo.min(y), hi.max(y)))
        };
        // the climb to 50 m, done before Auto starts, and back
        let alt = series("CTUN", "Alt", None);
        let at_auto = alt.xs.partition_point(|&x| x < 80.0);
        assert!(alt.ys[at_auto] > 49.5, "{} m at Auto", alt.ys[at_auto]);
        let (low, high) = extent(&alt.ys);
        assert!(
            low >= 0.0 && (49.5..50.5).contains(&high),
            "{low} to {high}"
        );
        // the climb rates in m/s, within the rates the script climbs and
        // descends at, and the barometer's height above the sea
        for field in ["DCRt", "CRt"] {
            let (low, high) = extent(&series("CTUN", field, None).ys);
            assert!(
                low >= -CLIMB_DOWN - 0.1 && high <= CLIMB_UP + 0.1,
                "{field}: {low} to {high}"
            );
            assert!(
                high > CLIMB_UP - 0.1,
                "{field} never climbs at {CLIMB_UP} m/s"
            );
        }
        let (field, sea) = (
            series("BARO", "Alt", None).ys,
            series("BARO", "AltAMSL", None).ys,
        );
        assert!(
            field
                .iter()
                .zip(&sea)
                .all(|(a, s)| ((s - a) - HOME_ALT).abs() < 1e-3)
        );
        // the speed over the legs, before and after the change
        let speed = series("GPS", "Spd", None);
        let peak = |from: f64, to: f64| {
            speed
                .xs
                .iter()
                .zip(&speed.ys)
                .filter(|(x, _)| (from..to).contains(*x))
                .fold(0.0f64, |peak, (_, &y)| peak.max(y))
        };
        assert!(peak(0.0, 60.0) < 0.2, "{}", peak(0.0, 60.0));
        assert!(
            (peak(90.0, 130.0) - 8.0).abs() < 0.3,
            "{}",
            peak(90.0, 130.0)
        );
        assert!(
            (peak(280.0, 420.0) - 10.0).abs() < 0.3,
            "{}",
            peak(280.0, 420.0)
        );
        // the satellites, in the fault and out of it
        let sats = series("GPS", "NSats", None);
        let (low, high) = extent(&sats.ys);
        assert!(close(low, 3.0) && close(high, 14.0), "{low} to {high}");
        let during = sats.xs.partition_point(|&x| x < 301.0);
        assert!(close(sats.ys[during], 5.0), "{}", sats.ys[during]);
        // the battery sags and the charge climbs
        let volt = series("BAT", "Volt", None).ys;
        let (low, _) = extent(&volt);
        assert!(
            volt[0] > 16.4 && low < volt[0] - 0.8,
            "{} to {low}",
            volt[0]
        );
        let charge = series("BAT", "CurrTot", None).ys;
        assert!(charge.windows(2).all(|w| w[1] >= w[0]));
        assert!(
            (1.5..2.6).contains(charge.last().unwrap()),
            "{:?}",
            charge.last()
        );
        // the lean is small and the yaw goes round the compass
        let (low, high) = extent(&series("ATT", "Roll", None).ys);
        assert!(low > -20.0 && high < 20.0, "{low} to {high}");
        let (low, high) = extent(&series("ATT", "Yaw", None).ys);
        assert!(low >= 0.0 && high < 360.0 && high - low > 180.0);
        // the accelerometers read gravity, each instance
        for instance in [0, 1] {
            let ys = series("IMU", "AccZ", Some(instance)).ys;
            assert!(ys.iter().all(|&a| (-14.0..-8.0).contains(&a)));
        }
        // the motors idle armed and rest disarmed
        let motor = series("RCOU", "C1", None).ys;
        assert!(close(motor[0], 1000.0));
        assert!(motor.iter().all(|&pwm| (1000.0..=2000.0).contains(&pwm)));
        assert!(motor.iter().any(|&pwm| pwm > 1350.0));
        let unused = series("RCOU", "C5", None).ys;
        assert!(unused.iter().all(|&pwm| close(pwm, 0.0)));
    }

    /// The mean of `|ATT.Pitch|` while `log` flies Auto.
    fn auto_pitch(log: &LoadedLog) -> f64 {
        let pitch = log
            .series(&SeriesKey {
                type_name: "ATT".into(),
                field: "Pitch".into(),
                instance: None,
            })
            .unwrap();
        let auto = log.modes.iter().find(|m| m.name == "Auto").unwrap().time;
        let rtl = log.modes.iter().find(|m| m.name == "RTL").unwrap().time;
        let during: Vec<f64> = pitch
            .xs
            .iter()
            .zip(&pitch.ys)
            .filter(|(x, _)| (auto..rtl).contains(*x))
            .map(|(_, y)| y.abs())
            .collect();
        during.iter().sum::<f64>() / during.len() as f64
    }

    #[test]
    fn flight_b_is_a_second_sortie_beside_the_first() {
        let a = loaded();
        let b = flight(Flight::B);
        assert_eq!(b.vehicle, Vehicle::Copter);
        let duration = b.duration().unwrap();
        assert!((480.0..560.0).contains(&duration), "{duration} s");

        // booted two hours after A
        let boot = |log: &LoadedLog| {
            let base = log.time_base.unwrap();
            base.gps_start_unix_ms - base.ms_offset
        };
        assert_eq!(boot(&b) - boot(&a), 7_200_000);

        // everything from arming on 12 s sooner after boot: the events
        // list's Armed at 33.25 s, the takeoff and Auto
        let event = |text: &str| b.events.iter().find(|e| e.text == text).unwrap().time;
        assert!(close(event("Armed"), 33.25), "{}", event("Armed"));
        assert!(close(event("Home set"), 22.05), "the boot as A's");
        let modes: Vec<(&str, f64)> = b.modes.iter().map(|m| (m.name.as_str(), m.time)).collect();
        assert_eq!(modes[1].0, "Loiter");
        assert!(
            close(modes[1].1, 33.0) && close(modes[2].1, 68.0),
            "{modes:?}"
        );
        // the takeoff's climb as A's: half a meter a hundred steps on
        let not_landed = (event("Not landed") * 100.0).round() / 100.0;
        assert!(close(not_landed, 38.99), "{not_landed}");

        // the survey 250 m north of A's, whose legs end 200 m north: the
        // tracks share the field and the way home, not the legs
        let (north_a, north_b) = (
            a.track.bounds().unwrap().north,
            b.track.bounds().unwrap().north,
        );
        let meters = (north_b - north_a) * M_PER_DEG;
        assert!((245.0..255.0).contains(&meters), "{meters} m");
        assert!(b.track.bounds().unwrap().south >= HOME_LAT - 0.0001);
        let mission = b
            .events
            .iter()
            .filter(|e| e.text.starts_with("Mission:"))
            .count();
        assert_eq!(mission, WAYPOINTS + 1, "the flight out is an item");

        // at 10 m/s from the start, with no change and no fault
        assert!(b.events.iter().all(|e| e.kind != EventKind::Error));
        assert!(b.events.iter().all(|e| e.kind != EventKind::Param));
        let speed = b
            .series(&SeriesKey {
                type_name: "GPS".into(),
                field: "Spd".into(),
                instance: None,
            })
            .unwrap();
        let peak = speed.ys.iter().fold(0.0f64, |peak, &y| peak.max(y));
        assert!((peak - 10.0).abs() < 0.3, "{peak}");
        let param = |name: &str| b.params.iter().find(|p| p.name == name).unwrap();
        assert!(close(param("WP_SPD").initial.into(), 10.0));
        assert!(param("WP_SPD").changes.is_empty());

        // its hover throttle drives the throttle, and is logged as the
        // parameter and in CTUN alike
        let hover = param("MOT_THST_HOVER").last();
        assert_eq!(hover.to_bits(), (B_HOVER as f32).to_bits());
        let logged = b
            .series(&SeriesKey {
                type_name: "CTUN".into(),
                field: "ThH".into(),
                instance: None,
            })
            .unwrap();
        assert!(logged.ys.iter().all(|&y| close(y, f64::from(hover))));

        // the stronger wind leans it harder into the legs, degrees more
        // on the whole of Auto, its flight out north, wind in roll, included
        let (pitch_a, pitch_b) = (auto_pitch(&a), auto_pitch(&b));
        assert!(pitch_b > pitch_a + 1.5, "{pitch_a} and {pitch_b}");

        // the dump as A's but for what the variant sets apart
        let differ: Vec<&str> = a
            .params
            .iter()
            .zip(&b.params)
            .filter(|(pa, pb)| {
                assert_eq!(pa.name, pb.name);
                pa.initial.to_bits() != pb.initial.to_bits()
            })
            .map(|(pa, _)| pa.name.as_str())
            .collect();
        // the retuned gains, the ground pressure and the PID logging, and
        // what the transit, the speed and the hover throttle set
        assert_eq!(
            differ,
            [
                "ATC_RAT_PIT_P",
                "ATC_RAT_RLL_P",
                "BARO1_GND_PRESS",
                "LOG_BITMASK",
                "MIS_TOTAL",
                "MOT_THST_HOVER",
                "WP_SPD"
            ]
        );
        assert!(close(param("MIS_TOTAL").initial.into(), 14.0));
        // PID logging back at 4.7.0's default
        assert_eq!(param("LOG_BITMASK").off_default(None), Some(false));
    }

    /// The series of `log`'s field `field` of `type_name`, a type logged
    /// as one instance.
    fn field_series(log: &LoadedLog, type_name: &str, field: &str) -> crate::model::Series {
        log.series(&SeriesKey {
            type_name: type_name.into(),
            field: field.into(),
            instance: None,
        })
        .unwrap()
    }

    /// When `log`'s mission reached each waypoint, as its "Mission: N WP"
    /// lines say, and the moment halfway along the leg between the
    /// second and the third, the first leg east of flight B.
    fn waypoint_times(log: &LoadedLog) -> (Vec<f64>, f64) {
        let times: Vec<f64> = log
            .events
            .iter()
            .filter(|e| e.text.starts_with("Mission:") && e.text.ends_with(" WP"))
            .map(|e| e.time)
            .collect();
        let leg = f64::midpoint(times[1], times[2]);
        (times, leg)
    }

    #[test]
    fn flight_b_logs_its_pid_loops_and_a_does_not() {
        let a = loaded();
        let b = flight(Flight::B);
        let series = |type_name: &str, field: &str| field_series(&b, type_name, field);
        let duration = b.duration().unwrap();
        for name in PID_TYPES {
            assert!(a.type_named(name).is_none(), "A logs {name}");
            let count = b.type_named(name).unwrap().count;
            assert!((count as f64 / 10.0 - duration).abs() < 1.0, "{name}");
        }
        // A's types are the ones both flights log
        assert!(
            a.types
                .iter()
                .all(|t| TYPES.iter().any(|&(name, ..)| name == t.name) || t.name == "FMT"),
            "{:?}",
            a.types.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
        // what only B has, as the side panel lists it, the four whole
        let only: Vec<String> = crate::tree::only_in(&b, &a)
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(only, ["PIDA", "PIDP", "PIDR", "PIDY"], "by name");
        // each dump's bitmask says whether the PID loops are logged
        for log in [&a, &b] {
            let bitmask = log.params.iter().find(|p| p.name == "LOG_BITMASK");
            let bitmask = f64::from(bitmask.unwrap().last());
            let pids = close(bitmask, f64::from(LOG_BITMASK | MASK_LOG_PID));
            assert!(pids || close(bitmask, f64::from(LOG_BITMASK)), "{bitmask}");
            assert_eq!(pids, log.type_named("PIDR").is_some(), "{}", log.name);
        }
        // B's file, its PID records among the streams, in time order
        assert!(b.timeline.windows(2).all(|w| w[1] >= w[0]));

        // the units and multipliers the tables give them: none but time's
        let pidr = b.type_named("PIDR").unwrap();
        let tar = pidr.field("Tar").unwrap();
        assert_eq!(tar.title(), "Tar");
        assert_eq!(tar.multiplier, None);
        assert_eq!(pidr.field("TimeUS").unwrap().title(), "TimeUS (s)");

        // the yaw rate holds the turn's rate, in rad/s, for over a second
        // after each waypoint, a 90-degree turn taking 1.5 s
        let turn = YAW_RATE.to_radians();
        let yaw = series("PIDY", "Tar");
        let (waypoints, leg) = waypoint_times(&b);
        assert_eq!(waypoints.len(), WAYPOINTS);
        for &at in &waypoints {
            let held = yaw
                .xs
                .iter()
                .zip(&yaw.ys)
                .filter(|&(&x, &y)| (at..at + 2.5).contains(&x) && (y.abs() - turn).abs() < 1e-5)
                .count();
            assert!(held >= 10, "{held} samples at the turn after {at} s");
        }
        // and none between turns, halfway along the first leg east
        let mid = yaw.xs.partition_point(|&x| x < leg);
        assert!(yaw.ys[mid].abs() < 0.02, "{}", yaw.ys[mid]);

        // the lean's steps spike roll and pitch, on either axis by the
        // heading, but briefly: never half the turn's rate for half a
        // second, five samples, which tells them from the yaw
        for name in ["PIDR", "PIDP"] {
            let ys = series(name, "Tar").ys;
            let longest = ys
                .split(|y| y.abs() <= turn / 2.0)
                .map(<[f64]>::len)
                .max()
                .unwrap_or(0);
            assert!(longest < 5, "{name}: {longest} samples");
            let peak = ys.iter().fold(0.0f64, |peak, y| peak.max(y.abs()));
            assert!(peak > 0.3, "{name}: no spike, {peak}");
        }
    }

    #[test]
    fn flight_b_s_pid_records_hold_together() {
        let b = flight(Flight::B);
        let series = |type_name: &str, field: &str| field_series(&b, type_name, field);
        let (_, leg) = waypoint_times(&b);

        // the vertical acceleration asked for, in m/s/s and down-positive:
        // the takeoff's climb speeding up at 1 m/s/s from 38 s, until it
        // reaches 2.5 m/s
        let accel = series("PIDA", "Tar");
        let climbing: Vec<f64> = accel
            .xs
            .iter()
            .zip(&accel.ys)
            .filter(|&(&x, _)| (38.5..40.0).contains(&x))
            .map(|(_, &y)| y)
            .collect();
        assert!(!climbing.is_empty());
        assert!(
            climbing.iter().all(|&y| (y + 1.0).abs() < 1e-3),
            "{climbing:?}"
        );
        let mid = accel.xs.partition_point(|&x| x < leg);
        assert!(accel.ys[mid].abs() < 0.05, "{}", accel.ys[mid]);

        // P is B's retuned roll gain times the error, record by record
        let gain = f64::from(
            b.params
                .iter()
                .find(|p| p.name == "ATC_RAT_RLL_P")
                .unwrap()
                .last(),
        );
        assert!(close(gain, f64::from(0.15f32)));
        let (err, p) = (series("PIDR", "Err").ys, series("PIDR", "P").ys);
        assert!(err.iter().zip(&p).all(|(e, p)| (p - gain * e).abs() < 1e-6));
        assert!(err.iter().any(|e| e.abs() > 1e-3), "an error to scale");
        // and Tar - Act is Err
        let (tar, act) = (series("PIDR", "Tar").ys, series("PIDR", "Act").ys);
        assert!(
            tar.iter()
                .zip(&act)
                .zip(&err)
                .all(|((t, a), e)| (t - a - e).abs() < 1e-6)
        );

        // the rate loops' integrators rest on the ground, before the
        // takeoff and after the landing, and work in the air
        let landed = b
            .events
            .iter()
            .find(|e| e.text == "Land complete")
            .unwrap()
            .time;
        let on_ground = |x: f64| x < 38.0 || x > landed;
        for name in ["PIDR", "PIDP", "PIDY"] {
            let i = series(name, "I");
            assert!(
                i.xs.iter()
                    .zip(&i.ys)
                    .all(|(&x, &y)| !on_ground(x) || close(y, 0.0)),
                "{name}"
            );
            assert!(i.ys.iter().any(|y| y.abs() > 1e-4), "{name}");
        }
        // the vertical loop's holds the hover throttle on the ground, and
        // in the air what the throttle asks beyond it, P and D (none):
        // in the climb's first seconds, the hover throttle's share of
        // 1 m/s/s, below the hover throttle
        let hover = b.params.iter().find(|p| p.name == "MOT_THST_HOVER");
        let hover = f64::from(hover.unwrap().last());
        let (i, p) = (series("PIDA", "I"), series("PIDA", "P").ys);
        for ((&x, &y), &p) in i.xs.iter().zip(&i.ys).zip(&p) {
            if on_ground(x) {
                assert!((y - hover).abs() < 1e-6, "{x} s: {y}");
            } else if (38.5..40.0).contains(&x) {
                assert!((y + p + hover / G).abs() < 1e-6, "{x} s: {y} and {p}");
            }
        }
    }

    #[test]
    fn both_flights_log_their_attitude_as_4_7_0_does() {
        for which in [Flight::A, Flight::B] {
            let log = flight(which);
            let series = |type_name: &str, field: &str| field_series(&log, type_name, field);
            let duration = log.duration().unwrap();
            for name in ["ATT", "ANG", "RATE"] {
                let count = log.type_named(name).unwrap().count;
                assert!((count as f64 / 10.0 - duration).abs() < 1.0, "{name}");
            }

            // ANG as ATT, from the same targets and estimates, with the
            // main loop's period
            for field in ["DesRoll", "Roll", "DesPitch", "Pitch", "DesYaw", "Yaw"] {
                assert_eq!(series("ATT", field).ys, series("ANG", field).ys, "{field}");
            }
            let dt = series("ANG", "Dt").ys;
            assert!(dt.iter().all(|d| (d - 1.0 / LOOP_RATE).abs() < 1e-4));

            // the yaw rate asked for in the turns, in deg/s
            let yaw = series("RATE", "YDes").ys;
            let peak = yaw.iter().fold(0.0f64, |peak, y| peak.max(y.abs()));
            assert!((peak - YAW_RATE).abs() < 1e-3, "{peak}");

            // the vertical acceleration in m/s/s under the 0.01 of its
            // centimeters, so the takeoff's 1 m/s/s up reads 0.01
            let rate = log.type_named("RATE").unwrap();
            for name in ["ADes", "A"] {
                let field = rate.field(name).unwrap();
                assert_eq!(field.title(), format!("{name} (m/s/s)"));
                assert_eq!(field.multiplier, Some(f64::from(0.01f32)));
            }
            let takeoff = which.variant().takeoff as f64 * DT;
            let accel = series("RATE", "ADes");
            let climbing: Vec<f64> = accel
                .xs
                .iter()
                .zip(&accel.ys)
                .filter(|&(&x, _)| (takeoff + 0.5..takeoff + 2.0).contains(&x))
                .map(|(_, &y)| y)
                .collect();
            assert!(!climbing.is_empty());
            assert!(
                climbing.iter().all(|y| (y - 0.01).abs() < 1e-6),
                "{climbing:?}"
            );

            // the throttle as CTUN logs it, and its slew rate: the arming's
            // step to SPIN_ARM, 10/s, filtered over the steps to the next
            // sample; the idle's steady throttle before the takeoff shows
            // none
            assert_eq!(series("RATE", "AOut").ys, series("CTUN", "ThO").ys);
            let slew = series("RATE", "AOutSlew");
            let arming_tick = which.variant().arm + 25;
            let armed = arming_tick as f64 * DT;
            let at_arming = slew.xs.partition_point(|&x| x <= armed);
            let step = slew.ys[at_arming];
            let decays = (10 - arming_tick % 10) % 10;
            let filtered =
                SPIN_ARM / DT * (DT / SLEW_LAG) * (1.0 - DT / SLEW_LAG).powi(decays as i32);
            assert!(
                (step - filtered).abs() < 1e-5,
                "{step}/s at {}",
                slew.xs[at_arming]
            );
            let idle = slew.xs.partition_point(|&x| x < armed + 2.0);
            assert!(slew.ys[idle] < 1e-9, "{}/s at the idle", slew.ys[idle]);
            assert!(slew.ys[at_arming - 1] < 1e-9, "before the arming");
        }
    }

    #[test]
    fn flight_b_s_rate_record_agrees_with_its_pid_loops() {
        let b = flight(Flight::B);
        let series = |type_name: &str, field: &str| field_series(&b, type_name, field).ys;
        let each =
            |ys: Vec<f64>, f: fn(f64) -> f64| -> Vec<f64> { ys.into_iter().map(f).collect() };
        // record by record, to the float fields' precision
        let agree = |logged: &[f64], expected: &[f64]| {
            logged.len() == expected.len()
                && logged
                    .iter()
                    .zip(expected)
                    .all(|(a, b)| (a - b).abs() <= 1e-6 * (1.0 + b.abs()))
        };
        // each rate as its loop's target and what was flown, in deg/s,
        // and the loop's output, P, I and D
        for (axis, pid) in [("R", "PIDR"), ("P", "PIDP"), ("Y", "PIDY")] {
            let desired = series("RATE", &format!("{axis}Des"));
            let target = each(series(pid, "Tar"), f64::to_degrees);
            assert!(agree(&desired, &target), "{pid}");
            let flown = each(series(pid, "Act"), f64::to_degrees);
            assert!(agree(&series("RATE", axis), &flown), "{pid}");
            let (p, i, d) = (series(pid, "P"), series(pid, "I"), series(pid, "D"));
            let output: Vec<f64> = p
                .iter()
                .zip(&i)
                .zip(&d)
                .map(|((p, i), d)| p + i + d)
                .collect();
            assert!(
                agree(&series("RATE", &format!("{axis}Out")), &output),
                "{pid}"
            );
        }
        // the vertical acceleration as the vertical loop's, up-positive,
        // once the 0.01 multiplier is taken back off
        let unscaled = |field: &str| each(series("RATE", field), |y| y / f64::from(0.01f32));
        assert!(agree(
            &unscaled("ADes"),
            &each(series("PIDA", "Tar"), |y| -y)
        ));
        assert!(agree(&unscaled("A"), &each(series("PIDA", "Act"), |y| -y)));
    }
}
