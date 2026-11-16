//! The little celebration when a port is really freed.
//!
//! Pure functions: the same seed and the same time always give the same
//! picture, so tests never depend on a clock or on randomness.

use std::time::Duration;

/// How long the burst lasts.
pub const DURATION: Duration = Duration::from_millis(1600);
const PARTICLES: usize = 70;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    pub x0: f32,
    pub y0: f32,
    /// Cells per second.
    pub vx: f32,
    pub vy: f32,
    /// Index into the glyph set and the colour set.
    pub glyph: usize,
    pub color: usize,
}

/// A tiny deterministic generator (xorshift), so no randomness crate is needed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A float in [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// The particles for one burst on a screen of this size.
pub fn burst(seed: u64, width: u16, height: u16) -> Vec<Particle> {
    // Scramble the seed first (splitmix64) so neighbouring seeds look
    // completely different, and so the generator never starts at zero.
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    let mut rng = Rng(if z == 0 { 0x2545_F491_4F6C_DD1D } else { z });
    let (w, h) = (width as f32, height as f32);
    (0..PARTICLES)
        .map(|_| Particle {
            x0: w / 2.0 + (rng.unit() - 0.5) * w * 0.2,
            y0: h * 0.55,
            vx: (rng.unit() - 0.5) * w * 0.9,
            vy: -(0.35 + rng.unit() * 0.9) * h * 1.4,
            glyph: (rng.next() % 6) as usize,
            color: (rng.next() % 4) as usize,
        })
        .collect()
}

/// Where a particle is `t` seconds after the burst. Gravity pulls it down.
pub fn position(p: &Particle, t: f32, height: u16) -> (f32, f32) {
    let gravity = height as f32 * 2.4;
    (p.x0 + p.vx * t, p.y0 + p.vy * t + 0.5 * gravity * t * t)
}

/// The cells to draw at time `t`: (x, y, glyph, colour), inside the screen,
/// one particle per cell (the later one wins).
pub fn cells(particles: &[Particle], t: f32, width: u16, height: u16) -> Vec<(u16, u16, usize, usize)> {
    if t < 0.0 || t > DURATION.as_secs_f32() {
        return Vec::new();
    }
    let mut out: Vec<(u16, u16, usize, usize)> = Vec::new();
    for p in particles {
        let (x, y) = position(p, t, height);
        if x < 0.0 || y < 0.0 || x >= width as f32 || y >= height as f32 {
            continue;
        }
        let (cx, cy) = (x as u16, y as u16);
        match out.iter_mut().find(|c| c.0 == cx && c.1 == cy) {
            Some(existing) => *existing = (cx, cy, p.glyph, p.color),
            None => out.push((cx, cy, p.glyph, p.color)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_burst() {
        assert_eq!(burst(42, 100, 30), burst(42, 100, 30));
        assert_ne!(burst(42, 100, 30), burst(43, 100, 30));
        assert_eq!(burst(0, 100, 30).len(), PARTICLES, "a zero seed still works");
    }

    #[test]
    fn particles_rise_then_fall_and_leave_the_screen() {
        let particles = burst(7, 100, 30);
        let early = cells(&particles, 0.15, 100, 30);
        assert!(early.len() > 20, "most particles are on screen early on: {}", early.len());
        // Early on they are above the starting line; by the end they have fallen.
        let rising = particles.iter().filter(|p| position(p, 0.15, 30).1 < p.y0).count();
        assert!(rising > PARTICLES / 2);
        let late = cells(&particles, 1.55, 100, 30);
        assert!(late.len() < early.len(), "gravity pulls most off the bottom");
    }

    #[test]
    fn nothing_is_drawn_outside_the_time_window_or_the_screen() {
        let particles = burst(1, 80, 24);
        assert!(cells(&particles, -0.1, 80, 24).is_empty());
        assert!(cells(&particles, DURATION.as_secs_f32() + 0.1, 80, 24).is_empty());
        for (x, y, glyph, color) in cells(&particles, 0.4, 80, 24) {
            assert!(x < 80 && y < 24 && glyph < 6 && color < 4);
        }
        assert!(cells(&particles, 0.4, 0, 0).is_empty());
    }

    #[test]
    fn each_cell_holds_at_most_one_particle() {
        let particles = burst(9, 40, 12);
        let drawn = cells(&particles, 0.3, 40, 12);
        for (i, a) in drawn.iter().enumerate() {
            assert!(!drawn[i + 1..].iter().any(|b| b.0 == a.0 && b.1 == a.1));
        }
    }
}
