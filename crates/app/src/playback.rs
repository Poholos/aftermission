//! Playback: what moves the playhead through the log's time by itself, at
//! a chosen speed. The playhead is the plot's, which every seek moves as
//! well, so playing goes on from wherever a seek puts it; this holds only
//! whether it plays and the time it last moved.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

/// The speeds on offer, as multiples of the log's own pace.
pub const SPEEDS: [f64; 8] = [0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0];

/// The longest a frame counts for, in seconds: a late frame, or the few
/// a hidden browser tab gets, moves the playhead this far at most rather
/// than leaping.
const MAX_FRAME: f64 = 0.25;

/// A speed, as its place in [`SPEEDS`]. Settings store it so, and a
/// stored place out of range reads as the fastest, so an old or edited
/// file cannot break the stepping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "usize", into = "usize")]
pub struct Speed(usize);

impl Default for Speed {
    /// 5x: the demo's eight and a half minutes in under two.
    fn default() -> Self {
        Self(4)
    }
}

impl From<usize> for Speed {
    fn from(index: usize) -> Self {
        Self(index.min(SPEEDS.len() - 1))
    }
}

impl From<Speed> for usize {
    fn from(speed: Speed) -> Self {
        speed.0
    }
}

impl Speed {
    /// Every speed, slowest first.
    pub fn all() -> impl Iterator<Item = Speed> {
        (0..SPEEDS.len()).map(Speed)
    }

    /// Log seconds per second.
    #[must_use]
    pub fn factor(self) -> f64 {
        SPEEDS.get(self.0).copied().unwrap_or(1.0)
    }

    /// The next speed up, or this one at the fastest.
    #[must_use]
    pub fn faster(self) -> Self {
        Self::from(self.0 + 1)
    }

    /// The next speed down, or this one at the slowest.
    #[must_use]
    pub fn slower(self) -> Self {
        Self(self.0.saturating_sub(1))
    }

    /// `5×`, `0.25×`
    #[must_use]
    pub fn label(self) -> String {
        format!("{}\u{d7}", self.factor())
    }
}

/// Whether the log plays, and the frame it last moved in.
#[derive(Debug, Default)]
pub struct Playback {
    playing: bool,
    /// egui's input time at the last advance, and the frame it was in;
    /// None until the first frame after Play, which only starts the clock.
    last: Option<(f64, u64)>,
    /// Playing when a drag of the slider began, to go on when it ends.
    resume_after_scrub: bool,
}

/// Where an advance put the playhead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    Moved(f64),
    /// At the range's last time, where playback stopped.
    Ended(f64),
}

impl Playback {
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Play from the next frame on: the frame that starts it counts no
    /// time.
    pub fn play(&mut self) {
        self.playing = true;
        self.last = None;
    }

    pub fn pause(&mut self) {
        self.playing = false;
        self.last = None;
        self.resume_after_scrub = false;
    }

    /// A drag of the slider starts: playback pauses while the hand moves
    /// the playhead, and remembers whether to go on.
    pub fn begin_scrub(&mut self) {
        let playing = self.playing;
        self.pause();
        self.resume_after_scrub = playing;
    }

    /// The drag ends: playback goes on if it was playing when it began.
    pub fn end_scrub(&mut self) {
        if std::mem::take(&mut self.resume_after_scrub) {
            self.play();
        }
    }

    /// The playhead after this frame: `playhead` moved by the time since
    /// the last frame, from egui's input time `now`, at `speed`, within
    /// `range`, which holds `playhead`. Nothing while paused, on the
    /// frame that starts playing, or when `frame` has not moved since the
    /// last advance, as when egui reruns a discarded pass. A frame counts
    /// for a quarter of a second at most. At the range's end playback
    /// stops, with [`Step::Ended`].
    pub fn advance(
        &mut self,
        speed: Speed,
        now: f64,
        frame: u64,
        playhead: f64,
        range: &RangeInclusive<f64>,
    ) -> Option<Step> {
        if !self.playing {
            return None;
        }
        let (then, then_frame) = self.last.replace((now, frame))?;
        if frame == then_frame {
            self.last = Some((then, then_frame));
            return None;
        }
        let elapsed = (now - then).clamp(0.0, MAX_FRAME);
        let to = playhead + elapsed * speed.factor();
        if to >= *range.end() {
            self.pause();
            Some(Step::Ended(*range.end()))
        } else {
            Some(Step::Moved(to))
        }
    }
}

/// Where Play starts: at `playhead`, which lies in `range`, or at the
/// range's first time when there is none yet or it stands at the end,
/// where the last playing stopped.
#[must_use]
pub fn start_from(playhead: Option<f64>, range: &RangeInclusive<f64>) -> f64 {
    match playhead {
        Some(time) if time < *range.end() => time,
        _ => *range.start(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Advance `playback` once per frame at the input times `times`, from
    /// `playhead`, and give where each frame left it.
    fn play_through(
        playback: &mut Playback,
        speed: Speed,
        times: &[f64],
        mut playhead: f64,
        range: &RangeInclusive<f64>,
    ) -> Vec<Option<Step>> {
        times
            .iter()
            .enumerate()
            .map(|(frame, &now)| {
                let step = playback.advance(speed, now, frame as u64, playhead, range);
                if let Some(Step::Moved(t) | Step::Ended(t)) = step {
                    playhead = t;
                }
                step
            })
            .collect()
    }

    #[test]
    fn the_playhead_moves_by_the_time_between_frames_at_the_speed() {
        let range = 0.0..=100.0;
        let one = Speed::from(2);
        let ten = Speed::from(5);
        assert_eq!((one.factor(), ten.factor()), (1.0, 10.0));

        // the frame that starts playing counts no time
        let mut playback = Playback::default();
        assert_eq!(playback.advance(one, 0.0, 0, 5.0, &range), None, "paused");
        playback.play();
        let steps = play_through(&mut playback, one, &[10.0, 10.1, 10.3], 5.0, &range);
        assert_eq!(steps[0], None);
        assert_eq!(steps[1], Some(Step::Moved(5.0 + (10.1 - 10.0))));
        assert_eq!(
            steps[2],
            Some(Step::Moved(5.0 + (10.1 - 10.0) + (10.3 - 10.1)))
        );

        // ten times as far at 10x
        let mut playback = Playback::default();
        playback.play();
        let steps = play_through(&mut playback, ten, &[0.0, 0.05, 0.1], 5.0, &range);
        let Some(Step::Moved(at)) = steps[2] else {
            panic!("{steps:?}");
        };
        assert!((at - 6.0).abs() < 1e-9, "{at}");

        // a late frame counts for a quarter of a second, a clock that went
        // back for nothing
        let mut playback = Playback::default();
        playback.play();
        let steps = play_through(&mut playback, ten, &[0.0, 3.0, 2.0], 5.0, &range);
        assert_eq!(steps[1], Some(Step::Moved(7.5)));
        assert_eq!(steps[2], Some(Step::Moved(7.5)));

        // a pause stops the clock: the time paused does not count
        playback.pause();
        assert_eq!(playback.advance(ten, 9.0, 10, 7.5, &range), None);
        playback.play();
        assert_eq!(playback.advance(ten, 20.0, 11, 7.5, &range), None);
        assert_eq!(
            playback.advance(ten, 20.1, 12, 7.5, &range),
            Some(Step::Moved(7.5 + (20.1 - 20.0) * 10.0))
        );
    }

    #[test]
    fn a_rerun_pass_of_a_frame_does_not_move_the_playhead_again() {
        let range = 0.0..=100.0;
        let mut playback = Playback::default();
        playback.play();
        let speed = Speed::from(2);
        assert_eq!(playback.advance(speed, 1.0, 7, 5.0, &range), None);
        assert_eq!(
            playback.advance(speed, 1.2, 8, 5.0, &range),
            Some(Step::Moved(5.0 + (1.2 - 1.0)))
        );
        // the same frame again, later in time: nothing, and the next frame
        // counts from the pass that moved
        assert_eq!(playback.advance(speed, 1.25, 8, 5.2, &range), None);
        assert_eq!(
            playback.advance(speed, 1.3, 9, 5.2, &range),
            Some(Step::Moved(5.2 + (1.3 - 1.2)))
        );
    }

    #[test]
    fn playing_stops_at_the_end_and_play_there_starts_over() {
        let range = 2.0..=10.0;
        let mut playback = Playback::default();
        playback.play();
        let speed = Speed::from(7);
        let steps = play_through(&mut playback, speed, &[0.0, 0.1, 0.2], 3.0, &range);
        assert_eq!(steps[1], Some(Step::Moved(9.0)));
        assert_eq!(steps[2], Some(Step::Ended(10.0)));
        assert!(!playback.is_playing());

        // Play at the end starts at the first time, as it does before
        // anything played; anywhere else, there
        assert_eq!(
            (
                start_from(Some(10.0), &range),
                start_from(None, &range),
                start_from(Some(4.5), &range),
            ),
            (2.0, 2.0, 4.5)
        );
    }

    #[test]
    fn a_drag_of_the_slider_pauses_and_goes_on_only_if_it_was_playing() {
        let mut playback = Playback::default();
        playback.play();
        playback.begin_scrub();
        assert!(!playback.is_playing());
        playback.end_scrub();
        assert!(playback.is_playing());

        playback.pause();
        playback.begin_scrub();
        playback.end_scrub();
        assert!(!playback.is_playing());
        // a pause during the drag, by a key, is kept
        playback.play();
        playback.begin_scrub();
        playback.pause();
        playback.end_scrub();
        assert!(!playback.is_playing());
    }

    #[test]
    fn the_speed_steps_within_its_range_and_reads_back_clamped() {
        let slowest = Speed::from(0);
        assert_eq!(slowest.slower(), slowest);
        assert_eq!(slowest.label(), "0.25\u{d7}");
        let fastest = Speed::from(SPEEDS.len() - 1);
        assert_eq!(fastest.faster(), fastest);
        assert_eq!(fastest.label(), "60\u{d7}");
        assert_eq!(Speed::default().label(), "5\u{d7}");
        assert_eq!(Speed::default().faster().label(), "10\u{d7}");
        assert_eq!(Speed::default().slower().label(), "2\u{d7}");
        assert_eq!(Speed::all().count(), SPEEDS.len());

        // stored as its place, and read back within range
        assert_eq!(ron::to_string(&Speed::default()).unwrap(), "4");
        let edited: Speed = ron::from_str("99").unwrap();
        assert_eq!(edited, fastest);
        assert_eq!(edited.slower().label(), "30\u{d7}");
    }
}
