//! Geometry: points, polylines and polygons (each possibly multi-part), plus hit testing.

use rustgis_geom::{Coord, Extent};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GeometryType {
    Point,
    Polyline,
    Polygon,
    /// A table with no geometry.
    None,
}

impl GeometryType {
    pub fn label(self) -> &'static str {
        match self {
            GeometryType::Point => "Point",
            GeometryType::Polyline => "Polyline",
            GeometryType::Polygon => "Polygon",
            GeometryType::None => "Table",
        }
    }
}

/// A geometry. Lines hold parts; polygons hold polygons of rings (first ring = exterior).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Geometry {
    Point(Coord),
    MultiPoint(Vec<Coord>),
    Line(Vec<Vec<Coord>>),
    Polygon(Vec<Vec<Vec<Coord>>>),
}

impl Geometry {
    pub fn kind(&self) -> GeometryType {
        match self {
            Geometry::Point(_) | Geometry::MultiPoint(_) => GeometryType::Point,
            Geometry::Line(_) => GeometryType::Polyline,
            Geometry::Polygon(_) => GeometryType::Polygon,
        }
    }

    /// Calls `f` with every coordinate.
    pub fn for_each_coord(&self, mut f: impl FnMut(Coord)) {
        match self {
            Geometry::Point(c) => f(*c),
            Geometry::MultiPoint(v) => v.iter().copied().for_each(f),
            Geometry::Line(parts) => parts.iter().flatten().copied().for_each(f),
            Geometry::Polygon(polys) => polys.iter().flatten().flatten().copied().for_each(f),
        }
    }

    pub fn coord_count(&self) -> usize {
        let mut n = 0;
        self.for_each_coord(|_| n += 1);
        n
    }

    pub fn extent(&self) -> Extent {
        let mut e = Extent::EMPTY;
        self.for_each_coord(|c| e.expand(c));
        e
    }

    /// A copy with every coordinate transformed.
    pub fn map_coords(&self, f: &impl Fn(Coord) -> Coord) -> Geometry {
        match self {
            Geometry::Point(c) => Geometry::Point(f(*c)),
            Geometry::MultiPoint(v) => Geometry::MultiPoint(v.iter().map(|c| f(*c)).collect()),
            Geometry::Line(parts) => Geometry::Line(parts.iter().map(|p| p.iter().map(|c| f(*c)).collect()).collect()),
            Geometry::Polygon(polys) => {
                Geometry::Polygon(polys.iter().map(|rings| rings.iter().map(|r| r.iter().map(|c| f(*c)).collect()).collect()).collect())
            }
        }
    }

    /// Whether `p` hits this geometry within `tol` (same units as the coordinates).
    pub fn hit(&self, p: Coord, tol: f64) -> bool {
        match self {
            Geometry::Point(c) => dist2(*c, p) <= tol * tol,
            Geometry::MultiPoint(v) => v.iter().any(|c| dist2(*c, p) <= tol * tol),
            Geometry::Line(parts) => parts.iter().any(|part| seg_dist2(part, p) <= tol * tol),
            Geometry::Polygon(polys) => {
                polys.iter().any(|rings| polygon_contains(rings, p) || rings.iter().any(|r| seg_dist2(r, p) <= tol * tol))
            }
        }
    }

    /// Whether any part of the geometry lies inside the rectangle (box selection).
    pub fn intersects_extent(&self, e: &Extent) -> bool {
        if !self.extent().intersects(e) {
            return false;
        }
        let mut any = false;
        self.for_each_coord(|c| any |= e.contains(c));
        if any {
            return true;
        }
        // A polygon that contains the whole box, or a box edge intersecting a polygon ring.
        if let Geometry::Polygon(polys) = self {
            if polys.iter().any(|rings| polygon_contains(rings, e.center())) {
                return true;
            }
            return polys.iter().any(|rings| {
                rings.iter().any(|ring| {
                    let n = ring.len();
                    (0..n).any(|i| {
                        let (Some(a), Some(b)) = (ring.get(i), ring.get((i + 1) % n)) else { return false };
                        segment_intersects_extent(*a, *b, e)
                    })
                })
            });
        }
        // A line crossing the box without a vertex inside it.
        if let Geometry::Line(parts) = self {
            return parts.iter().any(|part| {
                part.windows(2).any(|w| match w {
                    [a, b] => segment_intersects_extent(*a, *b, e),
                    _ => false,
                })
            });
        }
        false
    }

    /// A point for a label or a pop-up: the point itself, the middle vertex of the longest line
    /// part, or the centroid of the largest polygon (moved inside it if the centroid falls out).
    pub fn label_point(&self) -> Option<Coord> {
        match self {
            Geometry::Point(c) => Some(*c),
            Geometry::MultiPoint(v) => v.first().copied(),
            Geometry::Line(parts) => {
                let part = parts.iter().max_by_key(|p| p.len())?;
                part.get(part.len() / 2).copied()
            }
            Geometry::Polygon(polys) => {
                let rings = polys.iter().max_by(|a, b| {
                    let aa = a.first().map(|r| ring_area(r).abs()).unwrap_or(0.0);
                    let bb = b.first().map(|r| ring_area(r).abs()).unwrap_or(0.0);
                    aa.partial_cmp(&bb).unwrap_or(std::cmp::Ordering::Equal)
                })?;
                let outer = rings.first()?;
                let c = ring_centroid(outer)?;
                if polygon_contains(rings, c) {
                    return Some(c);
                }
                // Scan a horizontal line through the centroid for the widest inside span.
                let e = {
                    let mut e = Extent::EMPTY;
                    outer.iter().for_each(|c| e.expand(*c));
                    e
                };
                let steps = 64;
                let mut best: Option<(f64, Coord)> = None;
                for i in 1..steps {
                    let y = e.min_y + e.height() * i as f64 / steps as f64;
                    let mut run_start: Option<f64> = None;
                    for j in 0..=steps {
                        let x = e.min_x + e.width() * j as f64 / steps as f64;
                        let inside = polygon_contains(rings, Coord::new(x, y));
                        match (inside, run_start) {
                            (true, None) => run_start = Some(x),
                            (false, Some(s)) => {
                                let w = x - s;
                                if best.is_none_or(|(bw, _)| w > bw) {
                                    best = Some((w, Coord::new((s + x) / 2.0, y)));
                                }
                                run_start = None;
                            }
                            _ => {}
                        }
                    }
                }
                best.map(|(_, c)| c).or(Some(c))
            }
        }
    }
}

fn dist2(a: Coord, b: Coord) -> f64 {
    (a.x - b.x).powi(2) + (a.y - b.y).powi(2)
}

/// Squared distance from `p` to the closest segment of a polyline.
pub fn seg_dist2(line: &[Coord], p: Coord) -> f64 {
    let mut best = f64::INFINITY;
    for w in line.windows(2) {
        let [a, b] = w else { continue };
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 { (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
        best = best.min(dist2(p, Coord::new(a.x + t * dx, a.y + t * dy)));
    }
    if line.len() == 1
        && let Some(a) = line.first()
    {
        best = dist2(*a, p);
    }
    best
}

/// Whether a line segment intersects an extent (including crossing through it or touching boundaries).
pub fn segment_intersects_extent(p1: Coord, p2: Coord, e: &Extent) -> bool {
    if e.contains(p1) || e.contains(p2) {
        return true;
    }
    let min_x = p1.x.min(p2.x);
    let max_x = p1.x.max(p2.x);
    let min_y = p1.y.min(p2.y);
    let max_y = p1.y.max(p2.y);
    if min_x > e.max_x || max_x < e.min_x || min_y > e.max_y || max_y < e.min_y {
        return false;
    }

    let dx = p2.x - p1.x;
    let dy = p2.y - p1.y;
    let mut t0 = 0.0f64;
    let mut t1 = 1.0f64;

    let mut clip = |p: f64, q: f64| -> bool {
        if p == 0.0 {
            return q >= 0.0;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t1 {
                return false;
            }
            if r > t0 {
                t0 = r;
            }
        } else {
            if r < t0 {
                return false;
            }
            if r < t1 {
                t1 = r;
            }
        }
        true
    };

    clip(-dx, p1.x - e.min_x) && clip(dx, e.max_x - p1.x) && clip(-dy, p1.y - e.min_y) && clip(dy, e.max_y - p1.y) && t0 <= t1
}

/// Even-odd point-in-ring test.
pub fn ring_contains(ring: &[Coord], p: Coord) -> bool {
    let mut inside = false;
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (Some(a), Some(b)) = (ring.get(i), ring.get(j)) else { break };
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Inside the exterior ring and outside every hole.
pub fn polygon_contains(rings: &[Vec<Coord>], p: Coord) -> bool {
    let mut it = rings.iter();
    match it.next() {
        Some(outer) if ring_contains(outer, p) => !it.any(|h| ring_contains(h, p)),
        _ => false,
    }
}

/// Signed planar area (shoelace; positive = counter-clockwise).
pub fn ring_area(ring: &[Coord]) -> f64 {
    let n = ring.len();
    if n < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    for i in 0..n {
        let (Some(a), Some(b)) = (ring.get(i), ring.get((i + 1) % n)) else { break };
        s += a.x * b.y - b.x * a.y;
    }
    s / 2.0
}

/// Planar centroid of a ring (vertex average when degenerate).
pub fn ring_centroid(ring: &[Coord]) -> Option<Coord> {
    let a = ring_area(ring);
    if a.abs() < 1e-12 {
        let n = ring.len() as f64;
        if n == 0.0 {
            return None;
        }
        let (sx, sy) = ring.iter().fold((0.0, 0.0), |(x, y), c| (x + c.x, y + c.y));
        return Some(Coord::new(sx / n, sy / n));
    }
    let n = ring.len();
    let (mut cx, mut cy) = (0.0, 0.0);
    for i in 0..n {
        let (Some(p), Some(q)) = (ring.get(i), ring.get((i + 1) % n)) else { break };
        let f = p.x * q.y - q.x * p.y;
        cx += (p.x + q.x) * f;
        cy += (p.y + q.y) * f;
    }
    Some(Coord::new(cx / (6.0 * a), cy / (6.0 * a)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, s: f64) -> Vec<Coord> {
        vec![Coord::new(x0, y0), Coord::new(x0 + s, y0), Coord::new(x0 + s, y0 + s), Coord::new(x0, y0 + s), Coord::new(x0, y0)]
    }

    #[test]
    fn polygon_with_hole() {
        let g = Geometry::Polygon(vec![vec![square(0.0, 0.0, 10.0), square(4.0, 4.0, 2.0)]]);
        assert!(g.hit(Coord::new(1.0, 1.0), 0.0));
        assert!(!g.hit(Coord::new(5.0, 5.0), 0.0));
        assert!(g.hit(Coord::new(5.0, 5.0), 1.5)); // near the hole's edge
        let lp = g.label_point().unwrap();
        assert!(polygon_contains(&[square(0.0, 0.0, 10.0), square(4.0, 4.0, 2.0)], lp));
    }

    #[test]
    fn line_hit_and_box() {
        let g = Geometry::Line(vec![vec![Coord::new(0.0, 0.0), Coord::new(10.0, 0.0)]]);
        assert!(g.hit(Coord::new(5.0, 0.5), 1.0));
        assert!(!g.hit(Coord::new(5.0, 2.0), 1.0));
        assert!(g.intersects_extent(&Extent::new(4.0, -1.0, 6.0, 1.0)));
        assert!(!g.intersects_extent(&Extent::new(4.0, 1.0, 6.0, 2.0)));

        // Polygon crossing the box whose center is outside and vertices outside
        let poly = Geometry::Polygon(vec![vec![vec![
            Coord::new(-10.0, 7.0),
            Coord::new(20.0, 7.0),
            Coord::new(20.0, 20.0),
            Coord::new(-10.0, 20.0),
        ]]]);
        // Box is [0, 10] x [0, 10], center is (5, 5) which is outside y in [7, 20]
        assert!(poly.intersects_extent(&Extent::new(0.0, 0.0, 10.0, 10.0)));

        // Diagonal segment crossing through box corners
        assert!(segment_intersects_extent(Coord::new(-5.0, -5.0), Coord::new(15.0, 15.0), &Extent::new(0.0, 0.0, 10.0, 10.0)));
        // Segment collinear with box top edge
        assert!(segment_intersects_extent(Coord::new(-5.0, 10.0), Coord::new(15.0, 10.0), &Extent::new(0.0, 0.0, 10.0, 10.0)));
        // Segment outside box
        assert!(!segment_intersects_extent(Coord::new(-5.0, 12.0), Coord::new(15.0, 12.0), &Extent::new(0.0, 0.0, 10.0, 10.0)));
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        assert!(!ring_contains(&[], Coord::new(0.0, 0.0)));
        assert_eq!(ring_area(&[Coord::new(1.0, 1.0)]), 0.0);
        assert!(Geometry::Polygon(vec![]).label_point().is_none());
        assert!(Geometry::Line(vec![vec![]]).label_point().is_none());
    }
}
