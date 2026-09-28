//! The demo flight: an invented quadcopter survey of about eight and a
//! half minutes, written as a log through dflog's writer when asked, so
//! the repository holds no binary and the page downloads none. Every
//! value comes from the formulas here and a seeded noise generator;
//! nothing was read from a real log.
//!
//! The flight, in seconds since boot: the tables and the parameter dump;
//! GPS acquiring a fix while the copter sits on a field in Grundy County,
//! Iowa; at 45 s arming in Loiter and a takeoff to 50 m; at 80 s Auto
//! over six survey legs of 400 m, 40 m apart, in a crosswind, with a
//! speed change at 250 s and a GPS fault at 300 s; then RTL, Land, and
//! the disarm. The message types, formats, units and multipliers are
//! ArduPilot's, so the app reads it as it reads a real log.
//!
//! The flight path is integrated with sums, products and square roots
//! only, which every platform rounds the same way, so the record counts
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
/// What the status line says while the log is generated and indexed.
pub const LABEL: &str = "Generating the demo log";

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
/// `LogStructure.h` has them. Each is defined with its place in the
/// list, from 1, as its id.
const TYPES: [(&str, &str, &str, &str, &str); 17] = [
    ("MSG", "QZ", "TimeUS,Message", "s-", "F-"),
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
        "QccccCCCCB",
        "TimeUS,DesRoll,Roll,DesPitch,Pitch,DesYaw,Yaw,ErrRP,ErrYaw,AEKF",
        "sddddhhdd-",
        "FBBBBBBBB-",
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
        "QBffcfIffB",
        "TimeUS,I,Alt,Press,Temp,CRt,SMS,Offset,GndTemp,Health",
        "s#mPOnsmO-",
        "F-00B0C?0-",
    ),
    (
        "CTUN",
        "Qffffffefcfhhf",
        "TimeUS,ThI,ABst,ThO,ThH,DAlt,Alt,BAlt,DSAlt,SAlt,TAlt,DCRt,CRt,N",
        "s----mmmmmmnnz",
        "F----00B0B0BB0",
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
        "s#vvAaJOw%--",
        "F-000C/?0---",
    ),
    (
        "VIBE",
        "QBfffI",
        "TimeUS,IMU,VibeX,VibeY,VibeZ,Clip",
        "s#ooo-",
        "F-000-",
    ),
];

/// The unit ids the types above use, named as ArduPilot names them.
const UNITS: [(char, &str); 23] = [
    ('-', ""),
    ('?', "UNKNOWN"),
    ('#', "instance"),
    ('%', "%"),
    ('A', "A"),
    ('D', "deglatitude"),
    ('E', "rad/s"),
    ('J', "W.s"),
    ('O', "degC"),
    ('P', "Pa"),
    ('S', "satellites"),
    ('U', "deglongitude"),
    ('Y', "PWM"),
    ('a', "Ah"),
    ('d', "deg"),
    ('h', "degheading"),
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
const MULTIPLIERS: [(char, f32); 8] = [
    ('-', 0.0),
    ('?', 1.0),
    ('/', 3600.0),
    ('0', 1.0),
    ('B', 0.01),
    ('C', 0.001),
    ('F', 1e-6),
    ('G', 1e-7),
];

/// The parameter dump: name, value, default; NaN where the firmware
/// logs none. Every value invented; the off-default ones are the trims
/// and offsets a calibration leaves, the battery's setup, the flight
/// mode switch, the hover throttle the copter learned, the mission's
/// length and the survey speed.
const PARAMS: [(&str, f64, f64); 107] = [
    ("ACRO_RP_RATE", 360.0, 360.0),
    ("ACRO_YAW_P", 4.5, 4.5),
    ("AHRS_EKF_TYPE", 3.0, 3.0),
    ("AHRS_ORIENTATION", 0.0, 0.0),
    ("AHRS_TRIM_X", 0.0031, 0.0),
    ("AHRS_TRIM_Y", -0.0018, 0.0),
    ("ANGLE_MAX", 3000.0, 3000.0),
    ("ARMING_CHECK", 1.0, 1.0),
    ("ATC_ACCEL_P_MAX", 110_000.0, 110_000.0),
    ("ATC_ACCEL_R_MAX", 110_000.0, 110_000.0),
    ("ATC_ACCEL_Y_MAX", 27_000.0, 27_000.0),
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
    ("ATC_THR_MIX_MAN", 0.5, 0.5),
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
    ("FRAME_CLASS", 1.0, 1.0),
    ("FRAME_TYPE", 1.0, 1.0),
    ("FS_GCS_ENABLE", 1.0, 1.0),
    ("FS_THR_ENABLE", 1.0, 1.0),
    ("FS_THR_VALUE", 975.0, 975.0),
    ("GPS1_TYPE", 1.0, 1.0),
    ("GPS_HDOP_GOOD", 140.0, 140.0),
    ("INS_GYRO_FILTER", 20.0, 20.0),
    ("INS_ACCEL_FILTER", 20.0, 20.0),
    ("INS_USE", 1.0, 1.0),
    ("INS_USE2", 1.0, 1.0),
    ("LAND_SPEED", 50.0, 50.0),
    ("LAND_SPEED_HIGH", 0.0, 0.0),
    ("LOG_BITMASK", 176_126.0, 176_126.0),
    ("LOG_DISARMED", 0.0, 0.0),
    ("LOG_FILE_BUFSIZE", 200.0, 200.0),
    ("MIS_TOTAL", 13.0, 0.0),
    ("MOT_PWM_MIN", 1000.0, 1000.0),
    ("MOT_PWM_MAX", 2000.0, 2000.0),
    ("MOT_SPIN_ARM", SPIN_ARM, 0.1),
    ("MOT_SPIN_MIN", 0.15, 0.15),
    ("MOT_SPIN_MAX", 0.95, 0.95),
    ("MOT_THST_EXPO", 0.65, 0.65),
    ("MOT_THST_HOVER", HOVER, 0.35),
    ("MOT_HOVER_LEARN", 2.0, 2.0),
    ("PILOT_SPEED_UP", 250.0, 250.0),
    ("PILOT_SPEED_DN", 0.0, 0.0),
    ("PILOT_ACCEL_Z", 250.0, 250.0),
    ("PILOT_THR_BHV", 0.0, 0.0),
    ("PSC_ACCZ_P", 0.5, 0.5),
    ("PSC_ACCZ_I", 1.0, 1.0),
    ("PSC_POSXY_P", 1.0, 1.0),
    ("PSC_POSZ_P", 1.0, 1.0),
    ("PSC_VELXY_P", 2.0, 2.0),
    ("PSC_VELXY_I", 1.0, 1.0),
    ("PSC_VELZ_P", 5.0, 5.0),
    ("RC_SPEED", 490.0, 490.0),
    ("RCMAP_ROLL", 1.0, 1.0),
    ("RCMAP_PITCH", 2.0, 2.0),
    ("RCMAP_THROTTLE", 3.0, 3.0),
    ("RCMAP_YAW", 4.0, 4.0),
    ("RTL_ALT", 1500.0, 1500.0),
    ("RTL_ALT_FINAL", 0.0, 0.0),
    ("RTL_LOIT_TIME", 5000.0, 5000.0),
    ("RTL_SPEED", 0.0, 0.0),
    ("SCHED_LOOP_RATE", 400.0, 400.0),
    ("SERVO1_FUNCTION", 33.0, 33.0),
    ("SERVO2_FUNCTION", 34.0, 34.0),
    ("SERVO3_FUNCTION", 35.0, 35.0),
    ("SERVO4_FUNCTION", 36.0, 36.0),
    ("SYSID_THISMAV", 1.0, 1.0),
    ("THR_DZ", 100.0, 100.0),
    ("WP_YAW_BEHAVIOR", 2.0, 2.0),
    ("WPNAV_ACCEL", 250.0, 250.0),
    ("WPNAV_ACCEL_Z", 100.0, 100.0),
    ("WPNAV_RADIUS", 200.0, 200.0),
    ("WPNAV_SPEED", 800.0, 1000.0),
    ("WPNAV_SPEED_DN", 150.0, 150.0),
    ("WPNAV_SPEED_UP", 250.0, 250.0),
];

/// The demo log as bytes.
#[must_use]
pub fn bytes() -> Vec<u8> {
    let mut demo = Demo::new();
    demo.run();
    demo.w.into_bytes()
}

/// A time in the script as a tick.
const fn ticks(seconds: f64) -> u64 {
    (seconds * 100.0 + 0.5) as u64
}

/// Waypoint `index` of the survey, meters north and east of home: the
/// legs run east and back, each 40 m north of the last, starting at home.
/// Index 0 is home, where the mission starts; it is not flown to.
fn waypoint(index: usize) -> [f64; 2] {
    let leg = (index / 2) as f64;
    let east = if matches!(index % 4, 1 | 2) { LEG } else { 0.0 };
    [leg * SPACING, east]
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
    /// The speed over the legs, meters per second: `WPNAV_SPEED`.
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
    /// The throttle out, 0 to 1, and the current it draws, amperes.
    throttle: f64,
    current: f64,
    /// The charge drawn, mAh, and the energy, Wh.
    used_mah: f64,
    used_wh: f64,
    /// The receiver reports a fault.
    fault: bool,
    landed_at: Option<u64>,
}

impl Demo {
    fn new() -> Self {
        Self {
            w: LogWriter::new(),
            noise: Noise::new(),
            tick: 0,
            phase: Phase::Ground,
            pos: [0.0; 3],
            vel: [0.0; 3],
            acc: [0.0; 2],
            vz_acc: 0.0,
            vz_want: 0.0,
            wp: 0,
            cruise: 8.0,
            target_alt: 0.0,
            armed: false,
            airborne: false,
            // parked facing east, down the first leg
            att: [0.0, 0.0, 90.0],
            yaw_target: 90.0,
            rates: [0.0; 3],
            throttle: 0.0,
            current: 0.0,
            used_mah: 0.0,
            used_wh: 0.0,
            fault: false,
            landed_at: None,
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
    }

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
        for (id, (_, _, _, units, multipliers)) in (1u64..).zip(TYPES) {
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
        self.record("MSG", &[self.us(offset), Value::Str(text.into())]);
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
        match self.tick {
            0 => {
                self.tables();
                self.msg("ArduCopter V4.7.0 (0000000)", 100);
                self.msg("Frame: QUAD/X", 200);
                self.msg("RCOut: PWM:1-14", 300);
            }
            T_PARAMS..T_PARAMS_END => {
                let (name, value, default) = PARAMS[(self.tick - T_PARAMS) as usize];
                self.param(name, value, default, 0);
            }
            T_BOOT_MODE => self.mode(STABILIZE, REASON_INITIALIZED, 0),
            T_EKF_GPS => self.msg("EKF3 IMU0 is using GPS", 0),
            t if t == T_EKF_GPS + 10 => self.msg("EKF3 IMU1 is using GPS", 0),
            T_ORIGIN => self.msg("EKF3 IMU0 origin set", 0),
            t if t == T_ORIGIN + 5 => self.event(EV_HOME_SET, 0),
            T_ARM => self.mode(LOITER, REASON_RC, 0),
            t if t == T_ARM + 20 => self.msg("Arming motors", 0),
            t if t == T_ARM + 25 => {
                self.event(EV_ARMED, 0);
                self.armed = true;
            }
            T_TAKEOFF => {
                self.phase = Phase::Takeoff;
                self.target_alt = SURVEY_ALT;
            }
            T_AUTO => {
                self.mode(AUTO, REASON_RC, 0);
                self.event(EV_AUTO_ARMED, 100);
                // item 0 is home, where the copter is
                self.msg("Mission: 1 WP", 200);
                self.phase = Phase::Survey;
                self.wp = 1;
            }
            // a set from the ground station is logged twice
            t if t == T_SPEED || t == T_SPEED + 5 => {
                self.param("WPNAV_SPEED", 1000.0, 1000.0, 0);
                self.cruise = 10.0;
            }
            T_FAULT => {
                self.error(GPS_SUBSYSTEM, CODE_UNHEALTHY, 0);
                self.event(EV_LOST_GPS, 100);
                self.fault = true;
            }
            t if t == T_FAULT + FAULT_TICKS => {
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
                    waypoint(self.wp)
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
        self.power();
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
        if self.wp < WAYPOINTS {
            self.msg(&format!("Mission: {} WP", self.wp), 300);
        } else {
            self.msg(&format!("Mission: {WAYPOINTS} RTL"), 300);
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
        // the speed from which the copter can still stop at the target
        let speed = self.cruise.min((2.0 * ACCEL * distance).sqrt());
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
        let lean_east = if self.flying() {
            -0.8 - 0.25 * (t / 7.3).sin() - 0.15 * (t / 2.9).sin()
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

    /// The throttle for the thrust, and the battery it drains.
    fn power(&mut self) {
        self.throttle = if !self.armed {
            0.0
        } else if !self.flying() {
            SPIN_ARM
        } else {
            (HOVER * (G + self.vz_acc) / G / self.tilt()).clamp(0.1, 0.9)
        };
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
            self.att_record(560);
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
        let err_rp = 0.01 + n.gauss().abs() * 0.01;
        let err_yaw = 0.005 + n.gauss().abs() * 0.005;
        self.record(
            "ATT",
            &[
                self.us(offset),
                Value::F64(self.att[0]),
                Value::F64(roll),
                Value::F64(self.att[1]),
                Value::F64(pitch),
                Value::F64(heading_field(self.att[2])),
                Value::F64(yaw),
                Value::F64(err_rp),
                Value::F64(err_yaw),
                Value::U64(3),
            ],
        );
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
                Value::F64(pressure),
                Value::F64(temperature),
                Value::F64(climb),
                Value::U64(self.tick * 10),
                Value::F64(0.0),
                Value::F64(24.5),
                Value::U64(1),
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
        let centimeters = |v: f64| Value::I64((v * 100.0).round() as i64);
        self.record(
            "CTUN",
            &[
                self.us(offset),
                Value::F64(if flying { 0.5 } else { 0.0 }),
                Value::F64(boost),
                Value::F64(self.throttle),
                Value::F64(HOVER),
                Value::F64(self.target_alt.max(0.0)),
                Value::F64(self.pos[2]),
                Value::F64(self.baro_alt()),
                Value::F64(0.0),
                Value::F64(0.0),
                Value::F64(0.0),
                centimeters(self.vz_want),
                centimeters(self.vel[2]),
                Value::F64(0.5),
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
            (GPS_WEEK, GPS_MS_AT_BOOT + self.tick * 10)
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

/// A heading as the hundredths of a degree a `C` field holds, wrapped
/// after the rounding, so 359.998 is logged as 0.00 and never as 360.00.
fn heading_field(degrees: f64) -> f64 {
    (degrees * 100.0).round().rem_euclid(36_000.0) / 100.0
}

/// The heading from one point to another, degrees clockwise from north.
fn course(from: [f64; 3], to: [f64; 2]) -> f64 {
    (to[1] - from[1])
        .atan2(to[0] - from[0])
        .to_degrees()
        .rem_euclid(360.0)
}

/// A seeded xorshift generator: the same noise on every run.
struct Noise(u64);

impl Noise {
    fn new() -> Self {
        Self(0x9E37_79B9_7F4A_7C15)
    }

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
        LoadedLog::build(Log::from_source(bytes().into()), NAME.into())
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn the_demo_is_the_same_log_every_time() {
        let first = bytes();
        let again = bytes();
        assert!(first.iter().eq(again.iter()), "the bytes differ");
        assert!(
            (2_000_000..3_500_000).contains(&first.len()),
            "{} bytes",
            first.len()
        );
    }

    #[test]
    fn a_heading_is_logged_below_360() {
        assert!(close(heading_field(359.998), 0.0));
        assert!(close(heading_field(-0.001), 0.0));
        assert!(close(heading_field(-0.006), 359.99));
        assert!(close(heading_field(359.994), 359.99));
        assert!(close(heading_field(90.004), 90.0));
    }

    #[test]
    fn the_survey_folds_back_on_itself() {
        let points: Vec<[f64; 2]> = (0..WAYPOINTS).map(waypoint).collect();
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
        for name in ["ATT", "BARO", "CTUN", "RCOU"] {
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
        assert_eq!(title("BAT", "EnrgTot"), "EnrgTot (W.s)");
        assert_eq!(title("ATT", "Yaw"), "Yaw (degheading)");
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
        assert_eq!(lines(EventKind::Param), ["250.00 WPNAV_SPEED 800 -> 1000"]);

        let speed = log.params.iter().find(|p| p.name == "WPNAV_SPEED").unwrap();
        assert!(close(speed.initial.into(), 800.0));
        assert_eq!(speed.default, Some(1000.0));
        assert_eq!(speed.off_default(), Some(false), "set to the default");
        let off_default = log
            .params
            .iter()
            .filter(|p| p.off_default() == Some(true))
            .count();
        assert_eq!(off_default, 16);
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
}
