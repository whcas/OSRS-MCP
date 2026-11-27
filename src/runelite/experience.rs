//! The standard experience table for levels 1 to 99.
//!
//! Used only to check that a recorded level agrees with recorded experience.
//! It is the well-known table, generated from the published formula; the
//! tests pin several anchor values.

/// The highest level the table covers. Experience beyond level 99 does not
/// change the real level.
pub const MAX_LEVEL: u8 = 99;

/// The largest experience a skill can hold (200 million).
pub const MAX_EXPERIENCE: u32 = 200_000_000;

/// Experience required to reach each level; index 0 is level 1.
const LEVEL_THRESHOLDS: [u32; 99] = [
    0, 83, 174, 276, 388, 512, 650, 801, 969, 1_154, 1_358, 1_584, 1_833, 2_107, 2_411, 2_746,
    3_115, 3_523, 3_973, 4_470, 5_018, 5_624, 6_291, 7_028, 7_842, 8_740, 9_730, 10_824, 12_031,
    13_363, 14_833, 16_456, 18_247, 20_224, 22_406, 24_815, 27_473, 30_408, 33_648, 37_224, 41_171,
    45_529, 50_339, 55_649, 61_512, 67_983, 75_127, 83_014, 91_721, 101_333, 111_945, 123_660,
    136_594, 150_872, 166_636, 184_040, 203_254, 224_466, 247_886, 273_742, 302_288, 333_804,
    368_599, 407_015, 449_428, 496_254, 547_953, 605_032, 668_051, 737_627, 814_445, 899_257,
    992_895, 1_096_278, 1_210_421, 1_336_443, 1_475_581, 1_629_200, 1_798_808, 1_986_068,
    2_192_818, 2_421_087, 2_673_114, 2_951_373, 3_258_594, 3_597_792, 3_972_294, 4_385_776,
    4_842_295, 5_346_332, 5_902_831, 6_517_253, 7_195_629, 7_944_614, 8_771_558, 9_684_577,
    10_692_629, 11_805_606, 13_034_431,
];

/// The level implied by `experience`, capped at [`MAX_LEVEL`].
#[must_use]
pub fn level_for_experience(experience: u32) -> u8 {
    // The first threshold is 0, so at least one level is always reached, and
    // there are only 99 thresholds, so the count fits a `u8`.
    let reached = LEVEL_THRESHOLDS.partition_point(|&threshold| threshold <= experience);
    u8::try_from(reached).unwrap_or(MAX_LEVEL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_match_the_published_table() {
        let anchors: [(u8, u32); 7] = [
            (1, 0),
            (2, 83),
            (10, 1_154),
            (40, 37_224),
            (50, 101_333),
            (92, 6_517_253),
            (99, 13_034_431),
        ];
        for (level, xp) in anchors {
            assert_eq!(
                LEVEL_THRESHOLDS[usize::from(level) - 1],
                xp,
                "level {level}"
            );
        }
    }

    #[test]
    fn thresholds_strictly_increase() {
        assert!(LEVEL_THRESHOLDS.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn levels_change_exactly_at_thresholds() {
        assert_eq!(level_for_experience(0), 1);
        assert_eq!(level_for_experience(82), 1);
        assert_eq!(level_for_experience(83), 2);
        assert_eq!(level_for_experience(13_034_430), 98);
        assert_eq!(level_for_experience(13_034_431), 99);
        assert_eq!(level_for_experience(MAX_EXPERIENCE), 99);
    }
}
