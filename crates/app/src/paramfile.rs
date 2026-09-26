//! Export of the log's parameters as a `.param` file: one `NAME,VALUE`
//! line per parameter, the form Mission Planner and MAVProxy load.

use std::io::{self, Write};

use crate::params::{self, Param};
use crate::worker;

/// The file about to be written: a parameter's name and value per line.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamFile {
    /// The log's file name, for the header comment.
    pub log_name: String,
    /// Whether the values are those at boot rather than at the end of the
    /// log.
    pub boot: bool,
    /// Sorted by name, as the log's parameters are.
    pub values: Vec<(String, f32)>,
}

impl ParamFile {
    /// The file for the [`exportable`] `params`, with each one's value at
    /// boot when `boot` is set and its last value in the log otherwise.
    #[must_use]
    pub fn new(log_name: &str, params: &[Param], boot: bool) -> Self {
        let values = exportable(params)
            .map(|p| (p.name.clone(), if boot { p.initial } else { p.last() }))
            .collect();
        Self {
            log_name: log_name.to_string(),
            boot,
            values,
        }
    }

    /// Write the file to `out`: a `#` comment naming the log and which
    /// values these are, then one line per parameter, its value in the
    /// fewest digits that read back as the float32 the log stored. A
    /// control character in the log's name, which a Linux or macOS file
    /// name may hold, prints as `?`: a line break there would end the
    /// comment and put the rest of the name on a line a loader reads.
    pub fn write(&self, out: &mut impl Write) -> io::Result<()> {
        let which = if self.boot {
            "value at boot"
        } else {
            "last value"
        };
        let log_name: String = self
            .log_name
            .chars()
            .map(|c| if c.is_control() { '?' } else { c })
            .collect();
        writeln!(out, "# {log_name}: each parameter's {which} in the log")?;
        for (name, value) in &self.values {
            writeln!(out, "{name},{}", params::value_text(*value))?;
        }
        Ok(())
    }
}

/// The parameters a `.param` file can hold: a name no loader could read
/// back, from a damaged record, is left out. That is one with a comma, a
/// space, a control character or anything outside ASCII, or one that
/// would start a comment.
pub fn exportable(params: &[Param]) -> impl Iterator<Item = &Param> {
    params.iter().filter(|p| {
        !p.name.starts_with('#') && p.name.bytes().all(|b| b.is_ascii_graphic() && b != b',')
    })
}

/// The name the save dialog offers for `log_name`: `flight.bin` gives
/// `flight.param`, and `flight_boot.param` for the values at boot.
#[must_use]
pub fn file_name(log_name: &str, boot: bool) -> String {
    let stem = worker::file_stem(log_name);
    if boot {
        format!("{stem}_boot.param")
    } else {
        format!("{stem}.param")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(name: &str, initial: f32, changes: &[f32]) -> Param {
        Param {
            name: name.into(),
            initial,
            default: None,
            changes: changes.iter().map(|&v| (2.5, v)).collect(),
        }
    }

    fn text(file: &ParamFile) -> String {
        let mut out = Vec::new();
        file.write(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn values_are_the_last_or_those_at_boot_as_the_log_stored_them() {
        let params = [
            param("ANGLE_MAX", 3000.0, &[]),
            param("PSC_ACCZ_P", 0.3, &[0.25, 0.35]),
            param("SERVO_TEMP", -0.5, &[]),
            param("TINY_GAIN", 1e-7, &[]),
        ];
        let last = ParamFile::new("flight.bin", &params, false);
        assert_eq!(
            text(&last),
            "# flight.bin: each parameter's last value in the log\n\
             ANGLE_MAX,3000\n\
             PSC_ACCZ_P,0.35\n\
             SERVO_TEMP,-0.5\n\
             TINY_GAIN,0.0000001\n"
        );
        let boot = ParamFile::new("flight.bin", &params, true);
        assert_eq!(
            text(&boot),
            "# flight.bin: each parameter's value at boot in the log\n\
             ANGLE_MAX,3000\n\
             PSC_ACCZ_P,0.3\n\
             SERVO_TEMP,-0.5\n\
             TINY_GAIN,0.0000001\n"
        );
    }

    #[test]
    fn names_a_loader_cannot_read_back_are_left_out() {
        let params = [
            param("#NOTE", 1.0, &[]),
            param("BAD NAME", 1.0, &[]),
            param("BAD,NAME", 1.0, &[]),
            param("BAD\u{7}", 1.0, &[]),
            param("FLTMODE1", 5.0, &[]),
            param("NAMÉ", 1.0, &[]),
        ];
        let file = ParamFile::new("flight.bin", &params, false);
        assert_eq!(file.values, [("FLTMODE1".to_string(), 5.0)]);
        assert_eq!(exportable(&params).count(), 1);
    }

    #[test]
    fn a_line_break_in_the_log_name_stays_inside_the_comment() {
        let params = [param("FLTMODE1", 5.0, &[])];
        let file = ParamFile::new("a\nARMING_CHECK,0\r.bin", &params, false);
        assert_eq!(
            text(&file),
            "# a?ARMING_CHECK,0?.bin: each parameter's last value in the log\n\
             FLTMODE1,5\n"
        );
    }

    #[test]
    fn file_names_say_which_values_they_hold() {
        assert_eq!(file_name("flight.bin", false), "flight.param");
        assert_eq!(file_name("flight.bin", true), "flight_boot.param");
        assert_eq!(file_name("00000012.BIN", false), "00000012.param");
    }
}
