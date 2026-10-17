//! Shooting stars in the space around the globe: now and then a comet streaks across the view,
//! its tail fading behind it, and passes behind the globe. As in GeoLibre, one at a time, a few
//! seconds apart on average. Drawn by the view only (never in exports), and only while the map
//! shows its stars; between comets the view sleeps until the next one is due.

use egui::{Color32, Painter, Pos2, Rect, Stroke, pos2, vec2};
use rustgis_engine::map::Globe;
use rustgis_engine::render::GLOBE_GLOW;

/// Mean seconds between one comet ending and the next appearing (GeoLibre's 0.5 % chance per
/// 60 Hz frame).
const MEAN_GAP: f64 = 3.3;
/// Pieces the tail is drawn in, fading from the tail to the head.
const PIECES: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Comet {
    /// When it appeared (seconds).
    born: f64,
    /// Where it appeared, in points from the view's top left corner.
    x: f32,
    y: f32,
    /// Its direction (radians) and speed (points a second).
    angle: f32,
    speed: f32,
    /// Its tail's length in points, and its life in seconds.
    len: f32,
    life: f64,
}

impl Comet {
    /// Where its head is at `time`.
    fn head(&self, time: f64) -> Pos2 {
        let d = self.speed * (time - self.born) as f32;
        pos2(self.x + self.angle.cos() * d, self.y + self.angle.sin() * d)
    }

    /// How bright it is at `time`, from 1 when it appears to 0 at the end of its life.
    fn alpha(&self, time: f64) -> f32 {
        (1.0 - (time - self.born) / self.life).clamp(0.0, 1.0) as f32
    }
}

/// The view's comets.
#[derive(Default)]
pub(crate) struct Comets {
    comet: Option<Comet>,
    /// When the next comet appears (seconds), once the view has started its sky.
    next: Option<f64>,
    seed: u64,
}

impl Comets {
    /// A number in 0..1 (SplitMix64; the seed comes from the clock on first use).
    fn random(&mut self, time: f64) -> f32 {
        if self.seed == 0 {
            self.seed = time.to_bits() | 1;
        }
        self.seed = self.seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    }

    /// The wait for the next comet: exponential, so comets come at random.
    fn gap(&mut self, time: f64) -> f64 {
        let u = f64::from(self.random(time)).clamp(1e-6, 1.0);
        (-u.ln() * MEAN_GAP).clamp(0.2, 4.0 * MEAN_GAP)
    }

    /// Stops the comets (the view no longer shows stars); they start again later.
    pub fn clear(&mut self) {
        self.comet = None;
        self.next = None;
    }

    /// Moves the comets on to `time` in a `size` view, letting one go when its life is over or it
    /// has left the view and starting the next when it is due. Returns the seconds until the
    /// view must be drawn again for them.
    fn step(&mut self, time: f64, size: egui::Vec2) -> f64 {
        if !(time.is_finite() && size.x >= 1.0 && size.y >= 1.0) {
            return 1.0;
        }
        if let Some(c) = self.comet {
            let h = c.head(time);
            let gone = time - c.born >= c.life || h.x < -c.len || h.y < -c.len || h.x > size.x + c.len || h.y > size.y + c.len;
            if gone {
                self.comet = None;
                self.next = Some(time + self.gap(time));
            }
        }
        let next = match self.next {
            Some(n) => n,
            None => {
                let n = time + self.gap(time);
                self.next = Some(n);
                n
            }
        };
        if self.comet.is_none() && time >= next {
            self.comet = Some(Comet {
                born: time,
                x: self.random(time) * size.x,
                y: self.random(time) * size.y,
                angle: self.random(time) * std::f32::consts::TAU,
                speed: 360.0 + 480.0 * self.random(time),
                len: 150.0 + 200.0 * self.random(time),
                life: 1.33 + 0.67 * f64::from(self.random(time)),
            });
        }
        match self.comet {
            Some(_) => 0.0,
            None => (self.next.unwrap_or(time) - time).max(0.0),
        }
    }

    /// Draws the comet over space in `rect` and behind the globe `g`, and returns the seconds
    /// until the view must be drawn again for the comets.
    pub fn paint(&mut self, painter: &Painter, rect: Rect, g: &Globe, time: f64) -> f64 {
        let wait = self.step(time, rect.size());
        let Some(c) = self.comet else { return wait };
        let (cx, cy) = g.center();
        let centre = pos2(rect.min.x + cx as f32, rect.min.y + cy as f32);
        let r = g.disc_radius() as f32;
        let a = c.alpha(time);
        let head = c.head(time) + rect.min.to_vec2();
        let back = vec2(c.angle.cos(), c.angle.sin()) * c.len;
        let tail = head - back;
        // How much of a point shows: none on the globe, more through its atmosphere.
        let shows = |p: Pos2| {
            if !(r.is_finite() && r > 0.0) {
                return 1.0;
            }
            ((p.distance(centre) - r) / (r * (GLOBE_GLOW - 1.0)).max(1.0)).clamp(0.0, 1.0)
        };
        for i in 0..PIECES {
            let (t0, t1) = (i as f32 / PIECES as f32, (i + 1) as f32 / PIECES as f32);
            let (p0, p1) = (tail + back * t0, tail + back * t1);
            let k = a * t1 * shows(p0.lerp(p1, 0.5));
            if k > 0.01 {
                painter.line_segment([p0, p1], Stroke::new(1.5, Color32::from_white_alpha((255.0 * k) as u8)));
            }
        }
        let k = a * shows(head);
        if k > 0.01 {
            painter.circle_filled(head, 2.0, Color32::from_rgba_unmultiplied(200, 220, 255, (128.0 * k) as u8));
        }
        wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comets_come_one_at_a_time_and_go() {
        let mut c = Comets::default();
        let size = vec2(800.0, 600.0);
        // The first waits a while; the view sleeps until then.
        let wait = c.step(100.0, size);
        assert!(c.comet.is_none() && wait > 0.0 && wait <= 4.0 * MEAN_GAP);
        // It appears when due, and the view is drawn every frame while it flies.
        assert_eq!(c.step(100.0 + wait, size), 0.0);
        let first = c.comet.unwrap();
        assert!(first.x >= 0.0 && first.x <= 800.0 && first.len >= 150.0);
        assert_eq!(c.step(100.0 + wait + 0.5, size), 0.0);
        assert_eq!(c.comet, Some(first), "one at a time");
        // Its life over, it's gone and the next is a gap away.
        let later = c.step(100.0 + wait + 2.5, size);
        assert!(c.comet.is_none() && later > 0.0);
        // Many comets over a long time, each fading out.
        let (mut t, mut seen) = (200.0, 0);
        while t < 600.0 {
            c.step(t, size);
            if let Some(k) = c.comet {
                assert!((0.0..=1.0).contains(&k.alpha(t)));
                if k.born == t {
                    seen += 1;
                }
            }
            t += 1.0 / 60.0;
        }
        assert!((40..=250).contains(&seen), "{seen}");
        // Junk sizes and times are harmless.
        assert_eq!(c.step(f64::NAN, size), 1.0);
        assert_eq!(c.step(700.0, vec2(0.0, 0.0)), 1.0);
        c.clear();
        assert!(c.comet.is_none() && c.next.is_none());
    }
}
