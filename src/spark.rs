//! Tiny history graphs for the summary cards.

use std::collections::VecDeque;

/// How many scans of history to keep.
pub const CAPACITY: usize = 60;

/// The last `width` values as a one-line graph, scaled between the smallest
/// and largest value in that window. A flat line is drawn at mid height.
/// With fewer values than `width` the start is padded with spaces.
pub fn sparkline(values: &[u32], width: usize, levels: &[&str; 8]) -> String {
    if width == 0 {
        return String::new();
    }
    let window = &values[values.len().saturating_sub(width)..];
    let mut out = " ".repeat(width - window.len());
    let (Some(&min), Some(&max)) = (window.iter().min(), window.iter().max()) else {
        return out;
    };
    for &value in window {
        let step = if max == min {
            3
        } else {
            (((value - min) as f64 / (max - min) as f64) * 7.0).round() as usize
        };
        out.push_str(levels[step.min(7)]);
    }
    out
}

/// Recent counts, one entry per scan.
#[derive(Debug, Default, Clone)]
pub struct History {
    pub listening: VecDeque<u32>,
    pub exposed: VecDeque<u32>,
    pub connections: VecDeque<u32>,
    pub hidden: VecDeque<u32>,
}

impl History {
    pub fn record(&mut self, listening: usize, exposed: usize, connections: usize, hidden: usize) {
        for (series, value) in [
            (&mut self.listening, listening),
            (&mut self.exposed, exposed),
            (&mut self.connections, connections),
            (&mut self.hidden, hidden),
        ] {
            series.push_back(value as u32);
            while series.len() > CAPACITY {
                series.pop_front();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVELS: [&str; 8] = ["1", "2", "3", "4", "5", "6", "7", "8"];

    #[test]
    fn scales_between_the_lowest_and_highest_value() {
        assert_eq!(sparkline(&[0, 7], 2, &LEVELS), "18");
        assert_eq!(sparkline(&[10, 20, 30, 40], 4, &LEVELS), "1368");
    }

    #[test]
    fn a_flat_series_is_drawn_at_mid_height() {
        assert_eq!(sparkline(&[5, 5, 5], 3, &LEVELS), "444");
        assert_eq!(sparkline(&[0, 0], 2, &LEVELS), "44");
    }

    #[test]
    fn only_the_latest_values_that_fit_are_shown_and_short_series_are_padded() {
        assert_eq!(sparkline(&[0, 7, 0, 7, 0], 2, &LEVELS), "81");
        assert_eq!(sparkline(&[3], 4, &LEVELS), "   4");
        assert_eq!(sparkline(&[], 3, &LEVELS), "   ");
        assert_eq!(sparkline(&[1, 2], 0, &LEVELS), "");
    }

    #[test]
    fn history_keeps_a_fixed_window_per_series() {
        let mut h = History::default();
        for i in 0..(CAPACITY + 10) {
            h.record(i, 1, 2, 3);
        }
        assert_eq!(h.listening.len(), CAPACITY);
        assert_eq!(*h.listening.front().unwrap(), 10);
        assert_eq!(*h.listening.back().unwrap() as usize, CAPACITY + 9);
        assert!(h.exposed.iter().all(|v| *v == 1));
        assert_eq!(h.connections.len(), CAPACITY);
        assert_eq!(h.hidden.len(), CAPACITY);
    }
}
