// SPDX-License-Identifier: GPL-3.0-only
//! The panel's level meters: what they read and how they move.
//!
//! The ballistics are a standard peak programme meter's (IEC 60268-10 Type II),
//! not measured on an XDJ-AZ: a reading rises at once to each measurement's
//! peak and falls 24 dB in 2.8 s. A measurement covers about 23 ms, which
//! stands in for the standard's 10 ms integration time.
//!
//! The scale is not measured either. The panel prints dB marks without saying
//! what level they refer to, so 0 dB on the meters is taken to be
//! [`REFERENCE_DBFS`], the EBU R68 alignment level.

/// The digital level, in dB of full scale, that reads 0 dB on the meters.
pub const REFERENCE_DBFS: f32 = -18.0;
/// How fast a reading falls, in dB per second (24 dB in 2.8 s).
pub const FALL_DB_PER_S: f32 = 24.0 / 2.8;
/// Below the lowest mark on any meter: a reading here lights nothing.
pub const FLOOR_DB: f32 = -60.0;

/// The printed marks of a channel meter's segments, bottom to top.
pub const CHANNEL_MARKS: [f32; 11] = [
    -24.0, -15.0, -12.0, -9.0, -6.0, -3.0, 0.0, 3.0, 6.0, 9.0, 12.0,
];
/// The printed marks of the master meter's segments, left to right.
pub const MASTER_MARKS: [f32; 11] = [
    -24.0, -18.0, -15.0, -12.0, -9.0, -6.0, -3.0, 0.0, 3.0, 6.0, 9.0,
];

/// A sample peak (a fraction of full scale) in meter dB.
pub fn meter_db(peak: f32) -> f32 {
    if peak <= 0.0 {
        return FLOOR_DB;
    }
    (20.0 * peak.log10() - REFERENCE_DBFS).max(FLOOR_DB)
}

/// One meter's reading, in meter dB.
#[derive(Clone, Copy, Debug)]
pub struct Meter {
    pub db: f32,
}

impl Default for Meter {
    fn default() -> Self {
        Self { db: FLOOR_DB }
    }
}

impl Meter {
    /// A new measurement: the reading jumps up to it, never down.
    pub fn hit(&mut self, peak: f32) {
        self.db = self.db.max(meter_db(peak));
    }

    /// Let `dt` seconds pass.
    pub fn fall(&mut self, dt: f32) {
        self.db = (self.db - FALL_DB_PER_S * dt).max(FLOOR_DB);
    }

    /// How many segments are lit on a meter with these marks.
    pub fn lit(&self, marks: &[f32]) -> usize {
        marks.iter().take_while(|&&m| self.db >= m).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_lights_nothing() {
        let mut m = Meter::default();
        m.hit(0.0);
        assert_eq!(m.lit(&CHANNEL_MARKS), 0);
        assert_eq!(m.lit(&MASTER_MARKS), 0);
    }

    #[test]
    fn the_reference_level_reads_zero() {
        let mut m = Meter::default();
        m.hit(10f32.powf(REFERENCE_DBFS / 20.0));
        assert!(m.db.abs() < 0.01);
        // -24 up to 0 on a channel meter: 7 segments.
        assert_eq!(m.lit(&CHANNEL_MARKS), 7);
    }

    #[test]
    fn full_scale_lights_every_segment() {
        let mut m = Meter::default();
        m.hit(1.0);
        assert_eq!(m.lit(&CHANNEL_MARKS), CHANNEL_MARKS.len());
    }

    #[test]
    fn a_reading_falls_24_db_in_2_8_s() {
        let mut m = Meter::default();
        m.hit(1.0);
        let start = m.db;
        for _ in 0..28 {
            m.fall(0.1);
        }
        assert!((start - m.db - 24.0).abs() < 0.01);
        // A quieter measurement does not pull it down faster.
        let before = m.db;
        m.hit(0.0);
        assert_eq!(m.db, before);
    }
}
