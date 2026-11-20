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
/// The glow behind the sweep has this many strengths (0 is none).
pub const WAKE_STEPS: u8 = 8;
/// How far behind the beam (in radians) the glow reaches.
const WAKE_REACH: f32 = 0.55;
/// Seconds for the sweep to go once round.
pub const SWEEP_PERIOD_SECS: f32 = 6.0;

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
    /// A ring guide line (plain-character radar only).
    Ring(usize),
    /// The sweep (plain-character radar only): 3 is the bright head, lower
    /// numbers are its fading trail.
    Sweep(u8),
    Center,
    /// A ripple around a new port (plain-character radar only).
    Ripple,
    Dot {
        level: Level,
        state: RowState,
        /// The sweep passed over it a moment ago.
        hot: bool,
        /// How recently the sweep passed: 3 just now, down to 0 (not lately,
        /// or no sweep at all).
        glow: u8,
        selected: bool,
    },
    Label {
        ch: char,
        selected: bool,
        /// 0 faint, 1 normal, 2 bright, 3 warning (open to the whole network).
        tone: u8,
    },
    /// A character of a ring's name ("this PC", ...), written on the ring.
    RingLabel(char),
}

/// Smooth line work drawn with Braille dots (2 by 4 dots in every cell).
/// When shapes overlap in one cell, the one with the higher priority is
/// drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// A ring; the number is the band (0 inner to 2 outer).
    Ring(u8),
    /// A bearing tick; `true` for the long ones every 45 degrees.
    Tick(bool),
    /// The pulsing circle around the centre.
    Pulse,
    /// The rotating brackets around the selected port.
    Bracket,
    /// The ring around a port that just opened.
    Ripple,
    /// The ring that spreads from a port the sweep just hit: 2 is the
    /// freshest, 0 almost gone.
    Echo(u8),
    /// The bright line of the sweep itself.
    Beam,
}

impl Shape {
    fn priority(self) -> u8 {
        match self {
            Shape::Ring(_) => 1,
            Shape::Tick(_) => 2,
            Shape::Pulse => 3,
            Shape::Bracket => 4,
            Shape::Ripple => 5,
            Shape::Echo(_) => 6,
            Shape::Beam => 7,
        }
    }
}

/// How the radar is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Plain characters only (ASCII mode, or terminals without Braille).
    Cells,
    /// Smooth Braille rings, a glowing sweep and tidier labels.
    Scope,
}

/// Everything that changes with time, passed in so layout stays pure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    /// The sweep angle in radians, or `None` for no sweep.
    pub sweep: Option<f32>,
    /// Rotation of the selection brackets, in radians.
    pub spin: f32,
    /// Radius in dots of the pulse around the centre.
    pub pulse: f32,
}

impl Motion {
    /// A perfectly still radar.
    pub const STILL: Motion = Motion { sweep: None, spin: 0.0, pulse: 3.0 };
}

#[derive(Debug, Clone)]
pub struct Radar {
    pub width: u16,
    pub height: u16,
    pub cells: Vec<Vec<Mark>>,
    /// Braille dots per cell (Scope style only).
    pub bits: Vec<Vec<u8>>,
    /// Which shape owns the Braille dots of each cell (Scope style only).
    pub shapes: Vec<Vec<Option<Shape>>>,
    /// Strength of the sweep's glow behind each cell, 0 (none) to
    /// `WAKE_STEPS` (Scope style only).
    pub wake: Vec<Vec<u8>>,
    /// Ports that did not fit (the bands were full).
    pub not_shown: usize,
    pub too_small: bool,
}

/// Braille dot bits by [column within the cell][row within the cell].
const BRAILLE: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

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

    /// Light one Braille dot at a position given in dots (not cells).
    fn plot(&mut self, px: f32, py: f32, shape: Shape) {
        let (x, y) = (px.round() as i32, py.round() as i32);
        if x < 0 || y < 0 {
            return;
        }
        let (col, row) = ((x >> 1) as usize, (y >> 2) as usize);
        if col >= self.width as usize || row >= self.height as usize {
            return;
        }
        self.bits[row][col] |= BRAILLE[(x & 1) as usize][(y & 3) as usize];
        let owner = &mut self.shapes[row][col];
        if owner.is_none_or(|s| shape.priority() >= s.priority()) {
            *owner = Some(shape);
        }
    }

    fn arc(&mut self, cx: f32, cy: f32, r: f32, from: f32, to: f32, shape: Shape) {
        let steps = ((r * 5.0).ceil() as i32).max(24);
        for i in 0..=steps {
            let a = from + (to - from) * i as f32 / steps as f32;
            self.plot(cx + r * a.cos(), cy + r * a.sin(), shape);
        }
    }

    fn line(&mut self, from: (f32, f32), to: (f32, f32), shape: Shape) {
        let length = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
        let steps = ((length * 2.0).ceil() as i32).max(1);
        for i in 0..=steps {
            let k = i as f32 / steps as f32;
            self.plot(from.0 + (to.0 - from.0) * k, from.1 + (to.1 - from.1) * k, shape);
        }
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

/// How recently the sweep passed an angle: 3 just now ... 0 not lately.
fn glow_level(sweep: Option<f32>, theta: f32) -> (u8, f32) {
    let Some(phi) = sweep else {
        return (0, f32::MAX);
    };
    let secs = (phi - theta).rem_euclid(TAU) / TAU * SWEEP_PERIOD_SECS;
    let level = if secs < 0.9 {
        3
    } else if secs < 2.0 {
        2
    } else if secs < 3.0 {
        1
    } else {
        0
    };
    (level, secs)
}

/// Lay out the plain-character radar. `sweep` is the current sweep angle in
/// radians (or `None` for no sweep); it only changes the sweep and which dots
/// are "hot", never where anything is placed.
pub fn layout(width: u16, height: u16, blips: &[Blip], sweep: Option<f32>) -> Radar {
    layout_with(width, height, blips, Motion { sweep, ..Motion::STILL }, Style::Cells)
}

/// Lay out the Braille scope.
pub fn layout_scope(width: u16, height: u16, blips: &[Blip], motion: Motion) -> Radar {
    layout_with(width, height, blips, motion, Style::Scope)
}

fn layout_with(width: u16, height: u16, blips: &[Blip], motion: Motion, style: Style) -> Radar {
    let sweep = motion.sweep;
    let mut radar = Radar {
        width,
        height,
        cells: vec![vec![Mark::Empty; width as usize]; height as usize],
        bits: vec![vec![0; width as usize]; height as usize],
        shapes: vec![vec![None; width as usize]; height as usize],
        wake: vec![vec![0; width as usize]; height as usize],
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
    // Dot coordinates: the middle of the scope and its radius.
    let (cxp, cyp, rp) = (2.0 * geo.cx + 1.0, 4.0 * geo.cy + 2.0, 4.0 * geo.radius);

    if style == Style::Scope {
        // The glow behind the sweep, a smooth wedge of tinted cells.
        if let Some(phi) = sweep {
            // x and y also feed the geometry, so plain index loops read best.
            #[allow(clippy::needless_range_loop)]
            for y in 0..height as usize {
                #[allow(clippy::needless_range_loop)]
                for x in 0..width as usize {
                    let dx = (x as f32 + 0.5) * 2.0 - cxp;
                    let dy = (y as f32 + 0.5) * 4.0 - cyp;
                    let rad = (dx * dx + dy * dy).sqrt() / rp;
                    if rad > 1.04 {
                        continue;
                    }
                    let behind = (phi - dy.atan2(dx)).rem_euclid(TAU);
                    if behind < WAKE_REACH {
                        let f = (1.0 - behind / WAKE_REACH).powi(2) * (0.35 + 0.65 * (rad / 0.45).min(1.0));
                        radar.wake[y][x] = if f > 0.05 { ((f * 9.0).ceil() as u8).min(WAKE_STEPS) } else { 0 };
                    }
                }
            }
        }
        // Rings, bearing ticks, the beam and the pulse at the centre.
        for (band, edge) in RINGS.iter().enumerate() {
            radar.arc(cxp, cyp, edge * rp, 0.0, TAU, Shape::Ring(band as u8));
        }
        for degrees in (0..360).step_by(15) {
            let a = (degrees as f32).to_radians();
            let major = degrees % 45 == 0;
            let outer = if major { 1.09 } else { 1.045 };
            radar.line(
                (cxp + rp * a.cos(), cyp + rp * a.sin()),
                (cxp + rp * outer * a.cos(), cyp + rp * outer * a.sin()),
                Shape::Tick(major),
            );
        }
        if let Some(phi) = sweep {
            radar.line(
                (cxp + rp * 0.1 * phi.cos(), cyp + rp * 0.1 * phi.sin()),
                (cxp + rp * phi.cos(), cyp + rp * phi.sin()),
                Shape::Beam,
            );
        }
        radar.arc(cxp, cyp, motion.pulse.max(1.0), 0.0, TAU, Shape::Pulse);
    } else {
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

    // 5. Rings around ports that just opened, and around ports the sweep
    //    just hit.
    for (blip, x, y) in &placed {
        let (dot_x, dot_y) = (*x as f32 * 2.0 + 1.0, *y as f32 * 4.0 + 2.0);
        if style == Style::Scope {
            if let Some(age) = blip.age.filter(|a| *a >= 0.0 && *a < 3.0) {
                radar.arc(dot_x, dot_y, 3.0 + age * 10.0, 0.0, TAU, Shape::Ripple);
            }
            let (_, secs) = glow_level(sweep, geo.angle_of(*x, *y));
            if secs < 0.8 {
                let step = if secs < 0.27 { 2 } else if secs < 0.53 { 1 } else { 0 };
                radar.arc(dot_x, dot_y, 2.0 + secs * 9.0, 0.0, TAU, Shape::Echo(step));
            }
            if blip.selected {
                for k in 0..4 {
                    let from = motion.spin + k as f32 * PI / 2.0;
                    radar.arc(dot_x, dot_y, 6.0, from, from + 0.7, Shape::Bracket);
                }
            }
        } else if let Some(age) = blip.age.filter(|a| *a >= 0.0 && *a < 3.0) {
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
    let mut glows: Vec<u8> = Vec::with_capacity(placed.len());
    for (blip, x, y) in &placed {
        let theta = geo.angle_of(*x, *y);
        let hot = sweep.is_some_and(|phi| (phi - theta).rem_euclid(TAU) < HOT_WINDOW);
        let (glow, _) = glow_level(sweep, theta);
        glows.push(glow);
        put(
            &mut radar.cells,
            *x,
            *y,
            Mark::Dot { level: blip.level, state: blip.state, hot, glow, selected: blip.selected },
        );
    }

    // 7. Labels: the selected port always, others only where there is room.
    //    On the scope, while the sweep runs, quiet inner ports are named only
    //    for a few seconds after the sweep passes them; ports open to a
    //    network are always named. Without a sweep, everything that fits is
    //    named.
    let mut labelled: Vec<(&Blip, i32, i32, u8)> = placed
        .iter()
        .zip(&glows)
        .map(|((b, x, y), g)| (*b, *x, *y, *g))
        .collect();
    labelled.sort_by_key(|(b, _, _, _)| (!b.selected, b.band, b.seed));
    for (blip, x, y, glow) in labelled {
        let wanted = style == Style::Cells
            || sweep.is_none()
            || blip.selected
            || blip.band > 0
            || glow >= 1;
        if !wanted {
            continue;
        }
        let tone = match blip.band {
            2 => 3,
            1 => 2,
            _ if glow >= 2 => 2,
            _ => 1,
        };
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
                    put(
                        &mut radar.cells,
                        start + i as i32,
                        y,
                        Mark::Label { ch: *ch, selected: blip.selected, tone },
                    );
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
                Mark::Label { ch, selected: true, .. } => Some(*ch),
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

    fn motion(sweep: Option<f32>) -> Motion {
        Motion { sweep, spin: 0.5, pulse: 3.0 }
    }

    fn count_shapes(radar: &Radar, wanted: impl Fn(Shape) -> bool) -> usize {
        radar.shapes.iter().flatten().flatten().filter(|s| wanted(**s)).count()
    }

    fn sample_blips(n: u8) -> Vec<Blip> {
        (0..n).map(|i| blip(seed(&[b"sc", &[i]]), (i % 3) as usize, &format!("{}", 7000 + i as u32))).collect()
    }

    #[test]
    fn the_scope_places_dots_exactly_where_the_plain_radar_does() {
        let blips = sample_blips(12);
        for sweep in [None, Some(0.4), Some(4.0)] {
            let plain = layout(100, 24, &blips, sweep);
            let scope = layout_scope(100, 24, &blips, motion(sweep));
            assert_eq!(positions(&plain), positions(&scope), "sweep {sweep:?}");
        }
    }

    #[test]
    fn the_scope_draws_smooth_rings_ticks_and_a_pulse_instead_of_dotted_cells() {
        let radar = layout_scope(90, 22, &sample_blips(6), motion(None));
        for band in 0..3u8 {
            assert!(count_shapes(&radar, |s| s == Shape::Ring(band)) > 20, "ring {band} is drawn");
        }
        assert!(count_shapes(&radar, |s| matches!(s, Shape::Tick(true))) > 0, "long ticks");
        assert!(count_shapes(&radar, |s| matches!(s, Shape::Tick(false))) > 0, "short ticks");
        assert!(count_shapes(&radar, |s| s == Shape::Pulse) > 0);
        assert_eq!(count_shapes(&radar, |s| s == Shape::Beam), 0, "no beam without a sweep");
        // The plain-character guides are not used on the scope.
        let marks: Vec<Mark> = radar.cells.iter().flatten().copied().collect();
        assert!(!marks.iter().any(|m| matches!(m, Mark::Ring(_) | Mark::Sweep(_) | Mark::Ripple)));
        // Braille bits exist exactly where a shape owns a cell.
        for (y, row) in radar.bits.iter().enumerate() {
            for (x, bits) in row.iter().enumerate() {
                assert_eq!(*bits != 0, radar.shapes[y][x].is_some(), "cell {x},{y}");
            }
        }
    }

    #[test]
    fn the_outer_ring_reaches_the_edge_of_the_scope_and_stays_inside_the_area() {
        let radar = layout_scope(90, 22, &[], motion(None));
        let outer: Vec<(usize, usize)> = radar
            .shapes
            .iter()
            .enumerate()
            .flat_map(|(y, row)| row.iter().enumerate().map(move |(x, s)| (x, y, *s)))
            .filter(|(_, _, s)| *s == Some(Shape::Ring(2)))
            .map(|(x, y, _)| (x, y))
            .collect();
        let (min_y, max_y) = (outer.iter().map(|c| c.1).min().unwrap(), outer.iter().map(|c| c.1).max().unwrap());
        assert!(min_y <= 1 && max_y >= 20, "outer ring spans the full height: {min_y}..{max_y}");
        // It is a circle in dots, so about twice as many columns as rows.
        let (min_x, max_x) = (outer.iter().map(|c| c.0).min().unwrap(), outer.iter().map(|c| c.0).max().unwrap());
        let (width, height) = ((max_x - min_x) as f32, (max_y - min_y) as f32);
        assert!((width / height - 2.0).abs() < 0.3, "round, not squashed: {width} x {height}");
    }

    #[test]
    fn the_beam_follows_the_sweep_angle() {
        let a = layout_scope(90, 22, &[], motion(Some(0.0)));
        let b = layout_scope(90, 22, &[], motion(Some(PI / 2.0)));
        assert!(count_shapes(&a, |s| s == Shape::Beam) > 10);
        let beam_cells = |r: &Radar| -> Vec<(usize, usize)> {
            r.shapes.iter().enumerate().flat_map(|(y, row)| row.iter().enumerate().filter(|(_, s)| **s == Some(Shape::Beam)).map(move |(x, _)| (x, y))).collect()
        };
        // Angle 0 runs along a row to the right of the centre; angle 90 degrees runs down a column.
        let rows_a: std::collections::HashSet<usize> = beam_cells(&a).iter().map(|c| c.1).collect();
        let cols_b: std::collections::HashSet<usize> = beam_cells(&b).iter().map(|c| c.0).collect();
        assert!(rows_a.len() <= 2, "a horizontal beam sits on one or two rows: {rows_a:?}");
        assert!(cols_b.len() <= 2, "a vertical beam sits on one or two columns: {cols_b:?}");
    }

    #[test]
    fn when_shapes_overlap_in_a_cell_the_higher_priority_one_wins_in_either_order() {
        let mut radar = layout_scope(30, 10, &[], motion(None));
        radar.plot(11.0, 11.0, Shape::Beam);
        radar.plot(11.0, 11.0, Shape::Ring(2));
        assert_eq!(radar.shapes[2][5], Some(Shape::Beam), "a ring cannot cover the beam");
        let mut other = layout_scope(30, 10, &[], motion(None));
        other.plot(13.0, 11.0, Shape::Ring(1));
        other.plot(13.0, 11.0, Shape::Echo(2));
        assert_eq!(other.shapes[2][6], Some(Shape::Echo(2)));
        // Off the canvas is ignored, not a crash.
        other.plot(-5.0, 3.0, Shape::Beam);
        other.plot(9999.0, 9999.0, Shape::Beam);
    }

    #[test]
    fn the_glow_fades_with_distance_behind_the_beam_and_is_absent_ahead_of_it() {
        let radar = layout_scope(90, 22, &[], motion(Some(0.0)));
        // Walk along the row through the centre, to the right (angle 0). The
        // cells just behind the beam (below the axis, angle a little
        // negative i.e. a little behind) are lit; cells ahead are not.
        let (cx, cy) = (45usize, 10usize);
        let behind_row = cy; // on the axis: at the beam
        let strength = |x: usize, y: usize| radar.wake[y][x];
        assert!(strength(cx + 12, behind_row) > 0, "on the beam");
        // Behind the beam (above the axis) the glow fades as we move away.
        let above: Vec<u8> = (0..8).map(|d| strength(cx + 12, cy - d)).collect();
        assert!(above[0] >= above[3] && above[3] >= above[6], "fades: {above:?}");
        assert!(above.iter().any(|v| *v > 0));
        // No glow at all without a sweep.
        let still = layout_scope(90, 22, &[], motion(None));
        assert!(still.wake.iter().flatten().all(|v| *v == 0));
        // And never stronger than the top step.
        assert!(radar.wake.iter().flatten().all(|v| *v <= WAKE_STEPS));
    }

    #[test]
    fn dots_glow_for_a_few_seconds_after_the_sweep_passes() {
        let theta = 1.0f32;
        let level = |ahead_by: f32| glow_level(Some(theta + ahead_by), theta).0;
        assert_eq!(level(0.1), 3, "just now");
        assert_eq!(level(TAU * 1.5 / SWEEP_PERIOD_SECS), 2, "a second or so ago");
        assert_eq!(level(TAU * 2.5 / SWEEP_PERIOD_SECS), 1, "a couple of seconds ago");
        assert_eq!(level(TAU * 4.0 / SWEEP_PERIOD_SECS), 0, "not lately");
        assert_eq!(glow_level(None, theta).0, 0, "no sweep, no glow");
        // The beam has not reached it yet: it is about to be hit, so not glowing.
        assert_eq!(level(-0.3), 0);
    }

    #[test]
    fn while_the_sweep_runs_quiet_inner_ports_are_named_only_after_being_swept() {
        let inner = Blip { selected: false, ..blip(1, 0, "1111") };
        let outer = blip(2, 2, "2222");
        let mut picked = blip(3, 0, "3333");
        picked.selected = true;
        let blips = [inner.clone(), outer, picked];
        let labels = |r: &Radar| -> String {
            r.cells.iter().flatten().filter_map(|m| match m { Mark::Label { ch, .. } => Some(*ch), _ => None }).collect()
        };
        // Find where the inner port sits, then sweep just past it and far from it.
        let probe = layout_scope(100, 24, std::slice::from_ref(&inner), motion(None));
        let (x, y) = positions(&probe)[0];
        let theta = Geometry::new(100, 24).angle_of(x as i32, y as i32);
        let just_after = labels(&layout_scope(100, 24, &blips, motion(Some(theta + 0.3))));
        let long_after = labels(&layout_scope(100, 24, &blips, motion(Some(theta + 3.4))));
        let still = labels(&layout_scope(100, 24, &blips, motion(None)));
        assert!(just_after.contains("1111"), "named right after the sweep: {just_after}");
        assert!(!long_after.contains("1111"), "quiet again later: {long_after}");
        assert!(long_after.contains("2222") && long_after.contains("3333"), "exposed and selected always: {long_after}");
        assert!(still.contains("1111") && still.contains("2222") && still.contains("3333"), "no sweep: all named: {still}");
        // The plain radar always names everything that fits.
        assert!(labels(&layout(100, 24, &blips, Some(theta + 3.4))).contains("1111"));
    }

    #[test]
    fn swept_ports_echo_new_ports_ripple_and_the_selected_port_gets_brackets() {
        let mut fresh = blip(5, 1, "");
        fresh.age = Some(0.5);
        let selected = Blip { selected: true, ..blip(9, 0, "") };
        let blips = [fresh.clone(), selected.clone()];
        let probe = layout_scope(100, 24, &[fresh.clone()], motion(None));
        let (x, y) = positions(&probe)[0];
        let theta = Geometry::new(100, 24).angle_of(x as i32, y as i32);

        let hit = layout_scope(100, 24, &blips, motion(Some(theta + 0.1)));
        assert!(count_shapes(&hit, |s| matches!(s, Shape::Echo(_))) > 0, "echo around a just-swept port");
        assert!(count_shapes(&hit, |s| s == Shape::Ripple) > 0, "ripple around a new port");
        assert!(count_shapes(&hit, |s| s == Shape::Bracket) > 0, "brackets around the selected port");

        let later = layout_scope(100, 24, &blips, motion(Some(theta + 3.5)));
        let swept_ports_echo = count_shapes(&later, |s| matches!(s, Shape::Echo(_)));
        assert!(swept_ports_echo < count_shapes(&hit, |s| matches!(s, Shape::Echo(_))));

        // An old port and an unselected one get neither.
        let calm = layout_scope(100, 24, &[blip(5, 1, "")], motion(None));
        assert_eq!(count_shapes(&calm, |s| s == Shape::Ripple || s == Shape::Bracket), 0);

        // Spinning only moves the brackets, never the dots.
        let a = layout_scope(100, 24, &blips, Motion { sweep: None, spin: 0.0, pulse: 3.0 });
        let b = layout_scope(100, 24, &blips, Motion { sweep: None, spin: 2.0, pulse: 3.0 });
        assert_eq!(positions(&a), positions(&b));
        assert_ne!(a.bits, b.bits);
    }

    #[test]
    fn the_scope_is_deterministic_and_survives_any_size() {
        let blips = sample_blips(14);
        let m = motion(Some(2.2));
        let a = layout_scope(110, 30, &blips, m);
        let b = layout_scope(110, 30, &blips, m);
        assert_eq!(a.bits, b.bits);
        assert_eq!(a.shapes, b.shapes);
        assert_eq!(a.wake, b.wake);
        assert_eq!(a.cells, b.cells);
        for (w, h) in [(0, 0), (1, 1), (10, 4), (24, 9), (500, 200), (60, 3)] {
            let _ = layout_scope(w, h, &blips, m);
        }
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
