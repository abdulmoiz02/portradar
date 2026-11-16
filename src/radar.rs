//! The Radar tab: a pure layout of listening ports on concentric bands.
//!
//! The centre is this computer. The inner band holds ports only this
//! computer can reach, the middle band ports open on one network, and the
//! outer band ports open to the whole network.
//!
//! Everything here is a pure function: the same ports always land on the
//! same cells, whatever order they arrive in, and time is passed in
//! explicitly (the sweep angle and the age of new ports), so tests never
//! depend on a clock.

use std::f32::consts::{PI, TAU};

use crate::model::RowState;
use crate::verdict::Level;

/// The bands as (inner edge, outer edge) of the normalised radius.
const BANDS: [(f32, f32); 3] = [(0.12, 0.45), (0.52, 0.74), (0.81, 0.98)];
/// The ring lines are drawn at the outer edge of each band.
const RINGS: [f32; 3] = [0.47, 0.77, 1.0];
/// What each ring is called, from the centre outwards.
const RING_NAMES: [&str; 3] = ["this PC", "one network", "whole network"];
/// Smallest area a radar can be drawn in.
const MIN_WIDTH: u16 = 24;
const MIN_HEIGHT: u16 = 9;
/// Angular nudge when a spot is taken: 5 degrees.
const STEP: f32 = PI / 36.0;
/// A port counts as "just pinged" for this long (in radians) behind the sweep.
const HOT_WINDOW: f32 = 0.9;

#[derive(Debug, Clone)]
pub struct Blip {
    /// Stable identity, so the blip always lands in the same place.
    pub seed: u64,
    /// 0 = this PC only, 1 = one network, 2 = whole network.
    pub band: usize,
    pub label: String,
    pub level: Level,
    pub state: RowState,
    pub selected: bool,
    /// Seconds since the port opened, for the "new port" ripple.
    pub age: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mark {
    Empty,
    /// A ring guide line; the number is the band (0 inner to 2 outer).
    Ring(usize),
    /// The sweep: 3 is the bright head, lower numbers are its fading trail.
    Sweep(u8),
    Center,
    Ripple,
    Dot {
        level: Level,
        state: RowState,
        /// The sweep passed over it a moment ago.
        hot: bool,
        selected: bool,
    },
    Label {
        ch: char,
        selected: bool,
    },
    /// A character of a ring's name ("this PC", ...), written on the ring.
    RingLabel(char),
}

#[derive(Debug, Clone)]
pub struct Radar {
    pub height: u16,
    pub cells: Vec<Vec<Mark>>,
    /// Ports that did not fit (the bands were full).
    pub not_shown: usize,
    pub too_small: bool,
}

impl Radar {
    #[cfg(test)]
    pub fn at(&self, x: u16, y: u16) -> Mark {
        self.cells
            .get(y as usize)
            .and_then(|row| row.get(x as usize))
            .copied()
            .unwrap_or(Mark::Empty)
    }

    /// The cell of every dot.
    #[cfg(test)]
    pub fn dot_positions(&self) -> Vec<(u16, u16)> {
        let mut out = Vec::new();
        for (y, row) in self.cells.iter().enumerate() {
            for (x, mark) in row.iter().enumerate() {
                if matches!(mark, Mark::Dot { .. }) {
                    out.push((x as u16, y as u16));
                }
            }
        }
        out
    }
}

/// A stable 64-bit identity (FNV-1a) from a few facts about a port.
pub fn seed(parts: &[&[u8]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in *part {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0xff; // separator, so ("ab","c") differs from ("a","bc")
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

struct Geometry {
    cx: f32,
    cy: f32,
    /// Radius in rows. Columns are drawn twice as wide because a terminal
    /// cell is about twice as tall as it is wide.
    radius: f32,
    width: u16,
    height: u16,
}

impl Geometry {
    fn new(width: u16, height: u16) -> Geometry {
        let radius = (((height as f32) - 1.0) / 2.0).min(((width as f32) - 1.0) / 4.0);
        Geometry {
            cx: (width as f32 - 1.0) / 2.0,
            cy: (height as f32 - 1.0) / 2.0,
            radius,
            width,
            height,
        }
    }

    fn cell(&self, angle: f32, rows: f32) -> (i32, i32) {
        (
            (self.cx + 2.0 * rows * angle.cos()).round() as i32,
            (self.cy + rows * angle.sin()).round() as i32,
        )
    }

    fn inside(&self, x: i32, y: i32) -> bool {
        x >= 1 && y >= 0 && x < self.width as i32 - 1 && y < self.height as i32
    }

    /// Normalised radius of a cell.
    fn norm_radius(&self, x: i32, y: i32) -> f32 {
        let dx = (x as f32 - self.cx) / 2.0;
        let dy = y as f32 - self.cy;
        (dx * dx + dy * dy).sqrt() / self.radius
    }

    fn angle_of(&self, x: i32, y: i32) -> f32 {
        let dx = (x as f32 - self.cx) / 2.0;
        let dy = y as f32 - self.cy;
        dy.atan2(dx).rem_euclid(TAU)
    }
}

/// Nudges to try, closest first: (half-row step outward or inward, angle steps).
fn candidates() -> Vec<(i32, i32)> {
    let mut list: Vec<(i32, i32)> = Vec::new();
    for dr in -6..=6 {
        for k in -36..=36 {
            list.push((dr, k));
        }
    }
    list.sort_by_key(|&(dr, k)| (dr.abs() * 2 + k.abs(), k, dr));
    list
}

/// Lay out the radar. `sweep` is the current sweep angle in radians (or
/// `None` for no sweep); it only changes the sweep and which dots are "hot",
/// never where anything is placed.
pub fn layout(width: u16, height: u16, blips: &[Blip], sweep: Option<f32>) -> Radar {
    let mut radar = Radar {
        height,
        cells: vec![vec![Mark::Empty; width as usize]; height as usize],
        not_shown: 0,
        too_small: width < MIN_WIDTH || height < MIN_HEIGHT,
    };
    if radar.too_small {
        return radar;
    }
    let geo = Geometry::new(width, height);
    let put = |cells: &mut Vec<Vec<Mark>>, x: i32, y: i32, mark: Mark| {
        if x >= 0 && y >= 0 && (x as usize) < width as usize && (y as usize) < height as usize {
            cells[y as usize][x as usize] = mark;
        }
    };

    // 1. Ring guides.
    for (band, edge) in RINGS.iter().enumerate() {
        for i in 0..360 {
            let (x, y) = geo.cell((i as f32).to_radians(), edge * geo.radius);
            if geo.inside(x, y) && radar.cells[y as usize][x as usize] == Mark::Empty {
                put(&mut radar.cells, x, y, Mark::Ring(band));
            }
        }
    }

    // 2. The sweep and its fading trail.
    if let Some(angle) = sweep {
        for (trail, intensity) in [(3, 0.0f32), (2, 0.10), (1, 0.22), (1, 0.34)] {
            let a = angle - intensity;
            let mut r = 0.4;
            while r <= geo.radius {
                let (x, y) = geo.cell(a, r);
                if geo.inside(x, y) {
                    put(&mut radar.cells, x, y, Mark::Sweep(trail));
                }
                r += 0.5;
            }
        }
    }

    // 3. The centre ("this computer"), and the name of each ring written at
    //    its top, so nobody needs a legend.
    let (cx, cy) = (geo.cx.round() as i32, geo.cy.round() as i32);
    put(&mut radar.cells, cx, cy, Mark::Center);
    let mut reserved: Vec<(i32, i32)> = Vec::new();
    let mut last_label_row = i32::MIN / 2;
    for (band, name) in RING_NAMES.iter().enumerate() {
        // The two inner rings are named at the top. The outer one is named at
        // the bottom, because on a short screen its top row sits right next
        // to the middle ring's name.
        let y = if band == 2 {
            (geo.cy + RINGS[band] * geo.radius).round() as i32
        } else {
            (geo.cy - RINGS[band] * geo.radius).round() as i32
        };
        let len = name.chars().count() as i32;
        let start = cx - len / 2;
        let on_the_right_side = if band == 2 { y > cy + 1 } else { y < cy - 1 };
        let fits = y >= 0
            && y < geo.height as i32
            && on_the_right_side
            && (band == 2 || (y - last_label_row).abs() > 1)
            && (0..len).all(|i| geo.inside(start + i, y));
        if fits {
            if band != 2 {
                last_label_row = y;
            }
            for (i, ch) in name.chars().enumerate() {
                put(&mut radar.cells, start + i as i32, y, Mark::RingLabel(ch));
                reserved.push((start + i as i32, y));
            }
        }
    }

    // 4. Dots, placed in a fixed order (band, then identity), so the input
    //    order and the table's sort order never move anything.
    let mut order: Vec<&Blip> = blips.iter().collect();
    order.sort_by_key(|b| (b.band, b.seed));
    let nudges = candidates();
    let mut taken: Vec<(i32, i32)> = vec![(cx, cy)];
    taken.extend(reserved);
    let mut placed: Vec<(&Blip, i32, i32)> = Vec::new();

    for blip in order {
        let band = blip.band.min(2);
        let (lo, hi) = BANDS[band];
        let mid = (lo + hi) / 2.0 * geo.radius;
        let base_angle = (blip.seed % 7200) as f32 / 7200.0 * TAU;

        let spot = nudges.iter().find_map(|&(dr, k)| {
            let rows = mid + dr as f32 * 0.5;
            let (x, y) = geo.cell(base_angle + k as f32 * STEP, rows);
            let r = geo.norm_radius(x, y);
            (geo.inside(x, y) && r >= lo && r <= hi && !taken.contains(&(x, y))).then_some((x, y))
        });
        match spot {
            Some((x, y)) => {
                taken.push((x, y));
                placed.push((blip, x, y));
            }
            None => radar.not_shown += 1,
        }
    }

    // 5. Ripples around ports that just opened.
    for (blip, x, y) in &placed {
        if let Some(age) = blip.age.filter(|a| *a >= 0.0 && *a < 3.0) {
            let rows = 0.8 + age * 1.4;
            for i in 0..16 {
                let a = i as f32 / 16.0 * TAU;
                let rx = (*x as f32 + 2.0 * rows * a.cos()).round() as i32;
                let ry = (*y as f32 + rows * a.sin()).round() as i32;
                if geo.inside(rx, ry)
                    && !taken.contains(&(rx, ry))
                    && matches!(radar.cells[ry as usize][rx as usize], Mark::Empty | Mark::Ring(_))
                {
                    put(&mut radar.cells, rx, ry, Mark::Ripple);
                }
            }
        }
    }

    // 6. The dots themselves.
    for (blip, x, y) in &placed {
        let hot = sweep.is_some_and(|phi| {
            let theta = geo.angle_of(*x, *y);
            let behind = (phi - theta).rem_euclid(TAU);
            behind < HOT_WINDOW
        });
        put(
            &mut radar.cells,
            *x,
            *y,
            Mark::Dot { level: blip.level, state: blip.state, hot, selected: blip.selected },
        );
    }

    // 7. Labels: the selected port always, others only where there is room.
    let mut labelled = placed.clone();
    labelled.sort_by_key(|(b, _, _)| (!b.selected, b.band, b.seed));
    for (blip, x, y) in labelled {
        let chars: Vec<char> = blip.label.chars().collect();
        let len = chars.len() as i32;
        // Right of the dot, else left of it, with a one-cell gap.
        for start in [x + 2, x - 1 - len] {
            let fits = len > 0
                && (0..len).all(|i| {
                    let lx = start + i;
                    geo.inside(lx, y)
                        && !taken.contains(&(lx, y))
                        && !matches!(
                            radar.cells[y as usize][lx as usize],
                            Mark::Label { .. } | Mark::Dot { .. } | Mark::Center
                        )
                });
            if fits {
                for (i, ch) in chars.iter().enumerate() {
                    put(&mut radar.cells, start + i as i32, y, Mark::Label { ch: *ch, selected: blip.selected });
                }
                break;
            }
        }
    }
    radar
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blip(seed: u64, band: usize, label: &str) -> Blip {
        Blip {
            seed,
            band,
            label: label.to_string(),
            level: Level::Normal,
            state: RowState::Normal,
            selected: false,
            age: None,
        }
    }

    fn positions(radar: &Radar) -> Vec<(u16, u16)> {
        radar.dot_positions()
    }

    #[test]
    fn the_same_ports_always_land_on_the_same_cells() {
        let blips: Vec<Blip> = (0..8).map(|i| blip(seed(&[b"tcp", &[i]]), (i % 3) as usize, &format!("{}", 3000 + i as u32))).collect();
        let a = layout(60, 18, &blips, None);
        let b = layout(60, 18, &blips, None);
        assert_eq!(positions(&a), positions(&b));
        assert_eq!(positions(&a).len(), 8);
        assert_eq!(a.not_shown, 0);
    }

    #[test]
    fn input_order_and_sorting_never_move_anything() {
        let blips: Vec<Blip> = (0..12).map(|i| blip(seed(&[b"tcp", &[i]]), (i % 3) as usize, "p")).collect();
        let mut reversed = blips.clone();
        reversed.reverse();
        let mut rotated = blips.clone();
        rotated.rotate_left(5);
        let base = layout(70, 20, &blips, None);
        assert_eq!(base.cells, layout(70, 20, &reversed, None).cells);
        assert_eq!(base.cells, layout(70, 20, &rotated, None).cells);
    }

    #[test]
    fn adding_a_port_in_another_band_leaves_the_rest_where_they_were() {
        let inner: Vec<Blip> = (0..6).map(|i| blip(seed(&[b"in", &[i]]), 0, "x")).collect();
        let before = layout(70, 20, &inner, None);
        let mut with_outer = inner.clone();
        with_outer.push(blip(seed(&[b"out"]), 2, "y"));
        let after = layout(70, 20, &with_outer, None);
        for pos in positions(&before) {
            assert!(matches!(after.at(pos.0, pos.1), Mark::Dot { .. }), "dot at {pos:?} moved");
        }
        assert_eq!(positions(&after).len(), 7);
    }

    #[test]
    fn dots_sit_inside_their_own_band() {
        let blips: Vec<Blip> = (0..9).map(|i| blip(seed(&[b"b", &[i]]), (i % 3) as usize, "")).collect();
        let radar = layout(80, 24, &blips, None);
        let geo = Geometry::new(80, 24);
        let mut counts = [0usize; 3];
        for (x, y) in positions(&radar) {
            let r = geo.norm_radius(x as i32, y as i32);
            let band = BANDS.iter().position(|(lo, hi)| r >= *lo && r <= *hi).expect("inside some band");
            counts[band] += 1;
        }
        assert_eq!(counts, [3, 3, 3]);
    }

    #[test]
    fn the_sweep_changes_only_the_sweep_and_which_dots_are_hot() {
        let blips: Vec<Blip> = (0..6).map(|i| blip(seed(&[b"s", &[i]]), (i % 3) as usize, "")).collect();
        let still = layout(70, 20, &blips, None);
        let swept_a = layout(70, 20, &blips, Some(0.5));
        let swept_b = layout(70, 20, &blips, Some(3.5));
        assert_eq!(positions(&still), positions(&swept_a));
        assert_eq!(positions(&still), positions(&swept_b));
        let has_sweep = |r: &Radar| r.cells.iter().flatten().any(|m| matches!(m, Mark::Sweep(_)));
        assert!(!has_sweep(&still));
        assert!(has_sweep(&swept_a));
        assert!(has_sweep(&swept_b));
        // With no sweep nothing is hot.
        assert!(!still.cells.iter().flatten().any(|m| matches!(m, Mark::Dot { hot: true, .. })));
    }

    #[test]
    fn a_dot_is_hot_just_after_the_sweep_passes_it() {
        let one = vec![blip(77, 1, "")];
        let radar = layout(70, 20, &one, None);
        let (x, y) = positions(&radar)[0];
        let geo = Geometry::new(70, 20);
        let theta = geo.angle_of(x as i32, y as i32);
        let just_after = layout(70, 20, &one, Some(theta + 0.3));
        let long_after = layout(70, 20, &one, Some(theta + 3.0));
        let hot = |r: &Radar| matches!(r.at(x, y), Mark::Dot { hot: true, .. });
        assert!(hot(&just_after));
        assert!(!hot(&long_after));
    }

    #[test]
    fn a_crowded_band_still_shows_a_dozen_dots_at_80x24() {
        // The user's real machine: about a dozen local-only ports.
        let blips: Vec<Blip> = (0..12).map(|i| blip(seed(&[b"local", &[i]]), 0, &format!("{}", 5000 + i as u32))).collect();
        let radar = layout(76, 14, &blips, None);
        assert_eq!(radar.not_shown, 0, "all 12 should fit in a 76x14 radar");
        assert_eq!(positions(&radar).len(), 12);
    }

    #[test]
    fn dots_that_cannot_fit_are_counted_not_lost_silently() {
        let blips: Vec<Blip> = (0..200).map(|i| blip(seed(&[b"many", &(i as u32).to_le_bytes()]), 0, "")).collect();
        let radar = layout(40, 12, &blips, None);
        assert!(radar.not_shown > 0);
        assert_eq!(positions(&radar).len() + radar.not_shown, 200);
    }

    #[test]
    fn the_selected_port_is_always_labelled() {
        let mut blips: Vec<Blip> = (0..10).map(|i| blip(seed(&[b"l", &[i]]), (i % 3) as usize, "0000")).collect();
        blips[3].label = "9999".into();
        blips[3].selected = true;
        let radar = layout(70, 20, &blips, None);
        let text: String = radar
            .cells
            .iter()
            .flatten()
            .filter_map(|m| match m {
                Mark::Label { ch, selected: true } => Some(*ch),
                _ => None,
            })
            .collect();
        assert_eq!(text, "9999");
    }

    #[test]
    fn labels_never_cover_dots_or_each_other() {
        let blips: Vec<Blip> = (0..14).map(|i| blip(seed(&[b"c", &[i]]), (i % 3) as usize, "12345")).collect();
        let radar = layout(80, 24, &blips, None);
        assert_eq!(positions(&radar).len(), 14, "every dot is still there");
    }

    #[test]
    fn new_ports_get_a_ripple_that_grows_and_then_stops() {
        let mut fresh = blip(5, 1, "");
        fresh.age = Some(0.2);
        let young = layout(70, 20, &[fresh.clone()], None);
        assert!(young.cells.iter().flatten().any(|m| *m == Mark::Ripple));
        fresh.age = Some(4.0);
        let old = layout(70, 20, &[fresh], None);
        assert!(!old.cells.iter().flatten().any(|m| *m == Mark::Ripple));
    }

    #[test]
    fn each_ring_is_named_on_the_ring_and_dots_never_cover_the_names() {
        let blips: Vec<Blip> = (0..15).map(|i| blip(seed(&[b"n", &[i]]), (i % 3) as usize, "")).collect();
        let radar = layout(76, 20, &blips, Some(1.0));
        let rows: Vec<String> = radar
            .cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|m| match m {
                        Mark::RingLabel(c) => *c,
                        _ => ' ',
                    })
                    .collect::<String>()
            })
            .collect();
        for name in RING_NAMES {
            assert!(rows.iter().any(|r| r.contains(name)), "{name:?} missing:\n{rows:#?}");
        }
        assert_eq!(positions(&radar).len(), 15);
    }

    #[test]
    fn tiny_areas_are_reported_not_drawn() {
        assert!(layout(10, 4, &[blip(1, 0, "")], None).too_small);
        assert!(layout(23, 20, &[], None).too_small);
        assert!(!layout(24, 9, &[], None).too_small);
        // And nothing panics at absurd sizes.
        let _ = layout(0, 0, &[blip(1, 0, "")], Some(1.0));
        let _ = layout(500, 200, &[blip(1, 2, "9")], Some(1.0));
    }

    #[test]
    fn seeds_depend_on_every_part_and_their_boundaries() {
        assert_ne!(seed(&[b"ab", b"c"]), seed(&[b"a", b"bc"]));
        assert_ne!(seed(&[b"tcp", &[1]]), seed(&[b"tcp", &[2]]));
        assert_eq!(seed(&[b"x"]), seed(&[b"x"]));
    }
}
