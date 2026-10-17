//! Cutting geometry along the edge of a world projection: the meridian opposite its central one
//! (`lon_0 ± 180°`). A feature that crosses it is split, and the part past it moves a whole turn
//! to the other side of the map, so it isn't drawn as a band across the whole map (#209).
//! Edges stay straight in longitude/latitude, as everywhere else in the map.

use rustgis_data::Geometry;
use rustgis_geom::Coord;

/// How far inside the edge the pieces of a cut geometry are kept, in degrees (about 0.1 mm on the
/// ground), so each projects onto its own side of the map even when the projection normalises
/// longitudes (a datum shift goes through geocentric coordinates): well past proj4rs' 1e-12 rad
/// tolerance at ±180°, and small enough that only features narrower than it flatten against the
/// edge. Geometry that needs no cut is left exactly as it is instead, so it stays unchanged for
/// hit-testing and snapping as much as for drawing: a point or line on the edge itself is drawn
/// there as before. A projection whose datum shift bends its edge adds how far it strays
/// ([`cut_with_margin`]), and there points within that of the edge are pulled inside even when
/// nothing is cut.
const EDGE: f64 = 1e-9;

/// Longitudes (and central meridians) further out than this, three turns, aren't real data: such
/// geometry is left for the projection to drop, which also keeps the turn arithmetic small.
const FAR: f64 = 1080.0;

/// `g` (longitude/latitude) cut along the edge of a world projection centred on `lon0`, with every
/// piece moved by whole turns into `lon0 - 180 ..= lon0 + 180`.
pub fn cut(g: &Geometry, lon0: f64) -> Geometry {
    cut_with_margin(g, lon0, &|_| 0.0)
}

/// [`cut`], keeping cut pieces `margin(latitude)` degrees further inside the edge: for a projection
/// whose datum shift bends its edge off a meridian, by as far as it strays there.
pub fn cut_with_margin(g: &Geometry, lon0: f64, margin: &dyn Fn(f64) -> f64) -> Geometry {
    if lon0.is_nan() || lon0.abs() > FAR {
        return g.clone();
    }
    let t = Turn { lo: lon0 - 180.0, margin };
    match g {
        Geometry::Point(c) => Geometry::Point(fold(*c, t)),
        Geometry::MultiPoint(v) => Geometry::MultiPoint(v.iter().map(|c| fold(*c, t)).collect()),
        // A point the projection can't use breaks a line, as the renderer does, and the runs
        // either side are cut on their own.
        Geometry::Line(parts) => Geometry::Line(
            parts.iter().flat_map(|p| p.split(|c| !c.is_finite())).filter(|r| r.len() >= 2).flat_map(|r| cut_line(r, t)).collect(),
        ),
        Geometry::Polygon(polys) => Geometry::Polygon(polys.iter().flat_map(|p| cut_polygon(p, t)).collect()),
    }
}

/// The projection's own turn, `lo ..= lo + 360`, and how much further than [`EDGE`] inside its
/// edges cut pieces are kept, by latitude.
#[derive(Clone, Copy)]
struct Turn<'a> {
    lo: f64,
    margin: &'a dyn Fn(f64) -> f64,
}

/// The turns (360° windows from `lo`) that a run of points covers, or `None` when it can't be cut
/// (a missing coordinate, or longitudes no data has).
fn span(pts: &[Coord], t: Turn<'_>) -> Option<(i64, i64)> {
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for c in pts {
        if !c.x.is_finite() {
            return None;
        }
        min = min.min(c.x);
        max = max.max(c.x);
    }
    if pts.is_empty() || min < -FAR || max > FAR {
        return None;
    }
    let k0 = ((min - t.lo) / 360.0).floor() as i64;
    let k1 = ((max - t.lo) / 360.0).ceil() as i64 - 1;
    if k1 < k0 {
        // Every point on one edge (a meridian at 180° under `lon_0 = 0`): it belongs to both
        // neighbouring turns, so keep it in the one nearer the projection's own, on its own side.
        let k = if k1.abs() <= k0.abs() { k1 } else { k0 };
        return Some((k, k));
    }
    Some((k0, k1))
}

/// The longitudes window `k` covers: the turn `k` whole turns east of the projection's own.
fn window(lo: f64, k: i64) -> (f64, f64) {
    (lo + 360.0 * k as f64, lo + 360.0 * (k + 1) as f64)
}

/// A point of a cut piece in window `k`, moved into the projection's own turn and kept off its edge.
fn shift(c: Coord, t: Turn<'_>, k: i64) -> Coord {
    shift_by(c, t, k, (t.margin)(c.y))
}

/// [`shift`] with the margin at the point's latitude already worked out.
fn shift_by(c: Coord, t: Turn<'_>, k: i64, margin: f64) -> Coord {
    // Capped below half a turn, so the bounds can't cross whatever margin a caller passes.
    let inset = (EDGE + margin.max(0.0)).min(179.0);
    Coord::new((c.x - 360.0 * k as f64).clamp(t.lo + inset, t.lo + 360.0 - inset), c.y)
}

/// A point of geometry that needs no cut, all of it in window `k`: moved a turn when it lies in
/// another, left exactly as it is in the projection's own, unless it is within the margin of an
/// edge (see [`own_turn`]).
fn place(c: Coord, t: Turn<'_>, k: i64) -> Coord {
    if k == 0 { own_turn(c, t) } else { shift(c, t, k) }
}

/// A point already in the projection's own turn. Without a margin it stays exactly where it is (a
/// point on the edge itself is drawn there). Under a datum shift the real edge strays from the
/// cut meridian by the margin, so a point within that of either edge is pulled inside, by at
/// most the margin, rather than projecting onto the far side; it stays on its own side of the map.
fn own_turn(c: Coord, t: Turn<'_>) -> Coord {
    let margin = (t.margin)(c.y);
    if margin > 0.0 { shift_by(c, t, 0, margin) } else { c }
}

fn fold(c: Coord, t: Turn<'_>) -> Coord {
    if c.x.is_nan() || c.x.abs() > FAR {
        return c;
    }
    // A point on either edge (180° under `lon_0 = 0`) stays on its own side: exactly there
    // without a margin, just inside it with one.
    if (t.lo..=t.lo + 360.0).contains(&c.x) {
        return own_turn(c, t);
    }
    shift(c, t, ((c.x - t.lo) / 360.0).floor() as i64)
}

fn cut_line(part: &[Coord], t: Turn<'_>) -> Vec<Vec<Coord>> {
    let Some((k0, k1)) = span(part, t) else { return vec![part.to_vec()] };
    if k0 == k1 {
        return vec![part.iter().map(|c| place(*c, t, k0)).collect()];
    }
    let mut out = Vec::new();
    for k in k0..=k1 {
        let (a, b) = window(t.lo, k);
        let mut piece: Vec<Coord> = Vec::new();
        for w in part.windows(2) {
            match clip_segment(w[0], w[1], a, b) {
                Some((p, q)) => {
                    if piece.last() != Some(&p) {
                        keep_piece(&mut out, std::mem::take(&mut piece), t, k);
                        piece.push(p);
                    }
                    piece.push(q);
                }
                None => keep_piece(&mut out, std::mem::take(&mut piece), t, k),
            }
        }
        keep_piece(&mut out, piece, t, k);
    }
    out
}

fn keep_piece(out: &mut Vec<Vec<Coord>>, piece: Vec<Coord>, t: Turn<'_>, k: i64) {
    if piece.windows(2).any(|w| w[0] != w[1]) {
        out.push(piece.into_iter().map(|c| shift(c, t, k)).collect());
    }
}

/// The part of segment `p`–`q` with `a <= x <= b` (Liang–Barsky on x), `None` when it has no length.
fn clip_segment(p: Coord, q: Coord, a: f64, b: f64) -> Option<(Coord, Coord)> {
    let dx = q.x - p.x;
    if dx == 0.0 {
        return (a <= p.x && p.x <= b).then_some((p, q));
    }
    let (ta, tb) = ((a - p.x) / dx, (b - p.x) / dx);
    let (t0, t1) = (ta.min(tb).max(0.0), ta.max(tb).min(1.0));
    if t0 >= t1 {
        return None;
    }
    // Ends inside the window are kept exactly, so the next segment continues the same piece.
    let at = |t: f64| {
        if t <= 0.0 {
            p
        } else if t >= 1.0 {
            q
        } else {
            Coord::new((p.x + dx * t).clamp(a, b), p.y + (q.y - p.y) * t)
        }
    };
    Some((at(t0), at(t1)))
}

fn cut_polygon(rings: &[Vec<Coord>], t: Turn<'_>) -> Vec<Vec<Vec<Coord>>> {
    // Vertices the projection can't use are left out, so the ring is still cut.
    let finite: Vec<Vec<Coord>>;
    let rings = if rings.iter().flatten().all(|c| c.is_finite()) {
        rings
    } else {
        finite = rings.iter().map(|r| r.iter().copied().filter(|c| c.is_finite()).collect()).collect();
        &finite
    };
    let Some(outer) = rings.first() else { return Vec::new() };
    let Some((k0, k1)) = span(outer, t) else { return vec![rings.to_vec()] };
    if k0 == k1 {
        return vec![rings.iter().map(|r| r.iter().map(|c| place(*c, t, k0)).collect()).collect()];
    }
    let mut out = Vec::new();
    for k in k0..=k1 {
        let (a, b) = window(t.lo, k);
        // Only pieces with no area at all (lying along the cut) are dropped: a tiny feature
        // across the edge keeps both its pieces.
        let pieces = |r: &Vec<Coord>| clip_ring(r, a, b).into_iter().filter(|p| area(p) != 0.0);
        let mut polys: Vec<Vec<Vec<Coord>>> = pieces(outer).map(|p| vec![p]).collect();
        let boxes: Vec<Option<Bounds>> = polys.iter().map(|p| p.first().and_then(|r| bounds(r))).collect();
        // A hole's pieces go with the piece of the exterior they lie in (dropped only when the
        // exterior has no piece in this window).
        for hole in rings.iter().skip(1).flat_map(pieces) {
            if let Some(i) = home(&polys, &boxes, &hole, a, b) {
                polys[i].push(hole);
            }
        }
        out.extend(polys.into_iter().map(|p| p.into_iter().map(|r| r.into_iter().map(|c| shift(c, t, k)).collect()).collect()));
    }
    out
}

/// The parts of a ring with `a <= x <= b`, each a ring of its own. A ring that leaves the window
/// and comes back gives several pieces, closed along the cut only where the polygon is, so their
/// outlines don't draw lines along the map's edge between them.
fn clip_ring(r: &[Coord], a: f64, b: f64) -> Vec<Vec<Coord>> {
    let closed = r.len() > 1 && r.first() == r.last();
    let open = if closed { &r[..r.len() - 1] } else { r };
    let split = split_half(open, a, true).and_then(|east| {
        east.iter().try_fold(Vec::new(), |mut acc, p| {
            acc.extend(split_half(p, b, false)?);
            Some(acc)
        })
    });
    // Rings the split can't pair up (touching the cut in odd ways) fall back to Sutherland–Hodgman,
    // which keeps them whole, joined along the cut by edges that enclose nothing.
    let mut out = split.unwrap_or_else(|| vec![clip_half(&clip_half(open, |c| c.x >= a, a), |c| c.x <= b, b)]);
    if closed {
        for p in &mut out {
            if let Some(&first) = p.first() {
                p.push(first);
            }
        }
    }
    out
}

/// The parts of an open ring east (`east`) or west of meridian `x`. The ring is cut into runs on
/// the kept side, each from where it crosses in to where it crosses out. Along the meridian the
/// polygon's inside lies between the 1st and 2nd crossing (by latitude), the 3rd and 4th and so
/// on, so each run's exit is joined to the entry it is paired with. `None` when the crossings
/// don't pair up as an exit and an entry.
fn split_half(ring: &[Coord], x: f64, east: bool) -> Option<Vec<Vec<Coord>>> {
    let inside = |c: Coord| if east { c.x >= x } else { c.x <= x };
    let n = ring.len();
    let Some(start) = (0..n).find(|&i| !inside(ring[i])) else { return Some(vec![ring.to_vec()]) };
    let cross = |p: Coord, q: Coord| Coord::new(x, p.y + (q.y - p.y) * (x - p.x) / (q.x - p.x));
    let mut runs: Vec<Vec<Coord>> = Vec::new();
    let mut run: Option<Vec<Coord>> = None;
    for s in 0..n {
        let (p, q) = (ring[(start + s) % n], ring[(start + s + 1) % n]);
        match (inside(p), inside(q)) {
            (false, true) => {
                let e = cross(p, q);
                run = Some(if e == q { vec![q] } else { vec![e, q] });
            }
            (true, true) => run.get_or_insert_with(Vec::new).push(q),
            (true, false) => {
                let e = cross(p, q);
                if let Some(mut v) = run.take() {
                    if v.last() != Some(&e) {
                        v.push(e);
                    }
                    // A run along the meridian itself only touches it.
                    if v.iter().any(|c| c.x != x) {
                        runs.push(v);
                    }
                }
            }
            (false, false) => {}
        }
    }
    // Crossings along the meridian, by latitude: (y, is an entry, run).
    let mut crossings: Vec<(f64, bool, usize)> = Vec::with_capacity(runs.len() * 2);
    for (i, r) in runs.iter().enumerate() {
        crossings.push((r.first()?.y, true, i));
        crossings.push((r.last()?.y, false, i));
    }
    crossings.sort_by(|p, q| p.0.total_cmp(&q.0));
    let mut next = vec![usize::MAX; runs.len()];
    for pair in crossings.chunks(2) {
        let [(_, e0, r0), (_, e1, r1)] = pair else { return None };
        match (e0, e1) {
            (false, true) => next[*r0] = *r1,
            (true, false) => next[*r1] = *r0,
            _ => return None,
        }
    }
    let mut seen = vec![false; runs.len()];
    let mut out = Vec::new();
    for i in 0..runs.len() {
        let mut ring = Vec::new();
        let mut j = i;
        // Follow the runs round until the ring closes (an unpaired run gives up).
        while !*seen.get(j)? {
            seen[j] = true;
            ring.extend_from_slice(&runs[j]);
            j = next[j];
        }
        if j != i && !ring.is_empty() {
            return None;
        }
        if !ring.is_empty() {
            out.push(ring);
        }
    }
    Some(out)
}

fn clip_half(pts: &[Coord], inside: impl Fn(Coord) -> bool, x: f64) -> Vec<Coord> {
    let mut out = Vec::with_capacity(pts.len() + 4);
    let Some(&last) = pts.last() else { return out };
    let cross = |p: Coord, q: Coord| Coord::new(x, p.y + (q.y - p.y) * (x - p.x) / (q.x - p.x));
    let mut prev = last;
    for &cur in pts {
        match (inside(prev), inside(cur)) {
            (true, true) => out.push(cur),
            (true, false) => out.push(cross(prev, cur)),
            (false, true) => {
                out.push(cross(prev, cur));
                out.push(cur);
            }
            (false, false) => {}
        }
        prev = cur;
    }
    out
}

/// The exterior piece a hole piece lies in: the first that contains one of its vertices or edge
/// midpoints off the cut, else (rounding at the cut) the one whose box is nearest the hole's box
/// centre. `None` only when there is no piece.
///
/// `boxes` are the pieces' exteriors' bounds, worked out once per window: a point is only tested
/// against the rings whose box holds it.
fn home(polys: &[Vec<Vec<Coord>>], boxes: &[Option<Bounds>], hole: &[Coord], a: f64, b: f64) -> Option<usize> {
    let mids = hole.windows(2).map(|w| Coord::new((w[0].x + w[1].x) / 2.0, (w[0].y + w[1].y) / 2.0));
    let gap = |bx: &Option<Bounds>, c: Coord| match *bx {
        Some((x0, y0, x1, y1)) => (x0 - c.x).max(c.x - x1).max(0.0).hypot((y0 - c.y).max(c.y - y1).max(0.0)),
        None => f64::INFINITY,
    };
    hole.iter()
        .copied()
        .chain(mids)
        .filter(|c| c.x != a && c.x != b)
        .find_map(|at| polys.iter().zip(boxes).position(|(p, bx)| gap(bx, at) == 0.0 && p.first().is_some_and(|r| ring_contains(r, at))))
        .or_else(|| {
            let (x0, y0, x1, y1) = bounds(hole)?;
            let centre = Coord::new((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            (0..polys.len().min(boxes.len())).min_by(|&i, &j| gap(&boxes[i], centre).total_cmp(&gap(&boxes[j], centre)))
        })
}

/// A ring's bounds: min x, min y, max x, max y.
type Bounds = (f64, f64, f64, f64);

fn bounds(r: &[Coord]) -> Option<Bounds> {
    let first = r.first()?;
    Some(r.iter().fold((first.x, first.y, first.x, first.y), |(x0, y0, x1, y1), c| (x0.min(c.x), y0.min(c.y), x1.max(c.x), y1.max(c.y))))
}

/// Whether `p` is inside a ring (even–odd).
fn ring_contains(r: &[Coord], p: Coord) -> bool {
    let mut inside = false;
    let Some(&last) = r.last() else { return false };
    let mut prev = last;
    for &c in r {
        if (c.y > p.y) != (prev.y > p.y) && p.x < (prev.x - c.x) * (p.y - c.y) / (prev.y - c.y) + c.x {
            inside = !inside;
        }
        prev = c;
    }
    inside
}

/// Signed shoelace area, taken about the ring's first vertex so that rounding doesn't grow with
/// the coordinates: a ring lying along a meridian has exactly none.
fn area(r: &[Coord]) -> f64 {
    let (Some(&o), Some(&last)) = (r.first(), r.last()) else { return 0.0 };
    let mut prev = Coord::new(last.x - o.x, last.y - o.y);
    let mut s = 0.0;
    for &c in r {
        let c = Coord::new(c.x - o.x, c.y - o.y);
        s += prev.x * c.y - c.x * prev.y;
        prev = c;
    }
    s / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(x: f64, y: f64) -> Coord {
        Coord::new(x, y)
    }

    fn xs(g: &Geometry) -> (f64, f64) {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        g.for_each_coord(|c| {
            lo = lo.min(c.x);
            hi = hi.max(c.x);
        });
        (lo, hi)
    }

    #[test]
    fn a_polygon_across_the_edge_is_split_onto_both_sides() {
        // Central meridian 90°W: the edge is 90°E. A box from 60°E to 120°E crosses it.
        let square = Geometry::Polygon(vec![vec![vec![c(60.0, 40.0), c(120.0, 40.0), c(120.0, 60.0), c(60.0, 60.0), c(60.0, 40.0)]]]);
        let Geometry::Polygon(polys) = cut(&square, -90.0) else { panic!("a polygon") };
        assert_eq!(polys.len(), 2, "{polys:?}");
        let mut areas: Vec<f64> = polys.iter().map(|p| area(&p[0]).abs()).collect();
        areas.sort_by(f64::total_cmp);
        assert!((areas[0] - 600.0).abs() < 1e-3 && (areas[1] - 600.0).abs() < 1e-3, "{areas:?}");
        for p in &polys {
            let g = Geometry::Polygon(vec![p.clone()]);
            let (lo, hi) = xs(&g);
            assert!(lo >= -270.0 && hi <= 90.0, "inside the projection's turn: {lo}..{hi}");
        }
        // The eastern half moved a turn west, onto the left of the map.
        assert!(polys.iter().any(|p| p[0].iter().all(|c| c.x <= -239.0)));
    }

    #[test]
    fn a_ring_that_crosses_back_gives_separate_pieces() {
        // A "C" open to the east across 90°E (central meridian 90°W): its two arms past the edge
        // are pieces of their own, not one ring joined by an edge along the cut between them,
        // which an outline would draw along the map's edge.
        let c_shape = vec![
            c(70.0, 0.0),
            c(100.0, 0.0),
            c(100.0, 10.0),
            c(80.0, 10.0),
            c(80.0, 20.0),
            c(100.0, 20.0),
            c(100.0, 30.0),
            c(70.0, 30.0),
            c(70.0, 0.0),
        ];
        let Geometry::Polygon(polys) = cut(&Geometry::Polygon(vec![vec![c_shape]]), -90.0) else { panic!("a polygon") };
        assert_eq!(polys.len(), 3, "the C west of the edge and two arms east of it: {polys:?}");
        let total: f64 = polys.iter().map(|p| area(&p[0]).abs()).sum();
        assert!((total - (30.0 * 10.0 + 2.0 * 20.0 * 10.0)).abs() < 1e-3, "{total}");
        // No edge along the cut crosses the gap between the arms (10° to 20°).
        for p in &polys {
            for w in p[0].windows(2) {
                let on_cut = w[0].x == w[1].x && ((w[0].x - 90.0).abs() < 1e-6 || (w[0].x + 270.0).abs() < 1e-6);
                let spans_gap = w[0].y.min(w[1].y) < 20.0 - 1e-9 && w[0].y.max(w[1].y) > 10.0 + 1e-9;
                assert!(!(on_cut && spans_gap), "an edge along the cut bridges the arms: {w:?}");
            }
        }
    }

    #[test]
    fn a_tiny_feature_across_the_edge_keeps_both_pieces() {
        // About 2 cm by 10 cm across 90°E: each piece is about 1e-13 square degrees.
        let tiny = vec![c(89.9999999, 0.0), c(90.0000001, 0.0), c(90.0000001, 0.000001), c(89.9999999, 0.000001), c(89.9999999, 0.0)];
        let Geometry::Polygon(polys) = cut(&Geometry::Polygon(vec![vec![tiny]]), -90.0) else { panic!("a polygon") };
        assert_eq!(polys.len(), 2, "{polys:?}");
        assert!(polys.iter().all(|p| area(&p[0]) != 0.0), "{polys:?}");
        // A ring lying along the cut has no area at all, whatever its longitude.
        assert_eq!(area(&[c(90.0, 1.0), c(90.0, 5.0), c(90.0, 3.0), c(90.0, 1.0)]), 0.0);
    }

    #[test]
    fn rings_touching_the_cut_or_running_along_it_split_cleanly() {
        let areas = |g: Geometry| -> Vec<f64> {
            let Geometry::Polygon(polys) = g else { panic!("a polygon") };
            let mut a: Vec<f64> = polys.iter().map(|p| area(&p[0]).abs()).collect();
            a.sort_by(f64::total_cmp);
            a
        };
        // Crosses 90°E between 0° and 10°N, and touches it at one vertex (90, 20) from the west.
        let touching = vec![c(70.0, 0.0), c(100.0, 0.0), c(100.0, 10.0), c(85.0, 10.0), c(90.0, 20.0), c(70.0, 20.0), c(70.0, 0.0)];
        let whole = area(&touching).abs();
        let a = areas(cut(&Geometry::Polygon(vec![vec![touching]]), -90.0));
        assert_eq!(a.len(), 2, "{a:?}");
        assert!((a[0] - 100.0).abs() < 1e-6 && (a[0] + a[1] - whole).abs() < 1e-6, "{a:?} of {whole}");
        // An edge lying along the cut, from (90, 10) to (90, 20).
        let along = vec![c(70.0, 0.0), c(100.0, 0.0), c(100.0, 10.0), c(90.0, 10.0), c(90.0, 20.0), c(70.0, 20.0), c(70.0, 0.0)];
        let a = areas(cut(&Geometry::Polygon(vec![vec![along]]), -90.0));
        assert_eq!(a.len(), 2, "{a:?}");
        assert!((a[0] - 100.0).abs() < 1e-6 && (a[1] - 400.0).abs() < 1e-6, "{a:?}");
        // The same along-the-cut edge, met from the east side.
        let mirrored = vec![c(110.0, 0.0), c(80.0, 0.0), c(80.0, 10.0), c(90.0, 10.0), c(90.0, 20.0), c(110.0, 20.0), c(110.0, 0.0)];
        let a = areas(cut(&Geometry::Polygon(vec![vec![mirrored]]), -90.0));
        assert_eq!(a.len(), 2, "{a:?}");
        assert!((a[0] - 100.0).abs() < 1e-6 && (a[1] - 400.0).abs() < 1e-6, "{a:?}");
    }

    #[test]
    fn holes_follow_their_piece() {
        let outer = vec![c(60.0, 0.0), c(120.0, 0.0), c(120.0, 30.0), c(60.0, 30.0), c(60.0, 0.0)];
        let hole = vec![c(100.0, 10.0), c(100.0, 20.0), c(110.0, 20.0), c(110.0, 10.0), c(100.0, 10.0)];
        let Geometry::Polygon(polys) = cut(&Geometry::Polygon(vec![vec![outer, hole]]), -90.0) else { panic!("a polygon") };
        let with_hole: Vec<_> = polys.iter().filter(|p| p.len() == 2).collect();
        assert_eq!(with_hole.len(), 1, "the hole goes with the eastern piece only: {polys:?}");
        assert!(with_hole[0][1].iter().all(|c| (-260.0..=-250.0).contains(&c.x)));
    }

    #[test]
    fn a_line_across_the_edge_becomes_two_parts() {
        let river = Geometry::Line(vec![vec![c(80.0, 50.0), c(100.0, 52.0)]]);
        let Geometry::Line(parts) = cut(&river, -90.0) else { panic!("a line") };
        assert_eq!(parts.len(), 2, "{parts:?}");
        assert!((parts[0][1].x - 90.0).abs() < 1e-6 && parts[0][1].x < 90.0, "west part ends just inside the edge");
        assert!((parts[1][0].x + 270.0).abs() < 1e-6 && parts[1][0].x > -270.0, "east part starts just inside the other edge");
        assert!((parts[0][1].y - 51.0).abs() < 1e-9 && (parts[1][0].y - 51.0).abs() < 1e-9);
    }

    #[test]
    fn whole_world_shapes_survive() {
        // A world box and an Antarctica-like ring along the bottom: under a 0° projection
        // nothing changes, and under 90°W each is cut in two that together cover the same area.
        let world =
            Geometry::Polygon(vec![vec![vec![c(-180.0, -60.0), c(180.0, -60.0), c(180.0, 60.0), c(-180.0, 60.0), c(-180.0, -60.0)]]]);
        let ant =
            Geometry::Polygon(vec![vec![vec![c(-180.0, -70.0), c(180.0, -70.0), c(180.0, -90.0), c(-180.0, -90.0), c(-180.0, -70.0)]]]);
        for (g, want) in [(&world, 360.0 * 120.0), (&ant, 360.0 * 20.0)] {
            for lon0 in [0.0, -90.0, -54.0, 150.0] {
                let Geometry::Polygon(polys) = cut(g, lon0) else { panic!("a polygon") };
                let total: f64 = polys.iter().map(|p| area(&p[0]).abs()).sum();
                assert!((total - want).abs() < 1e-3, "lon0 {lon0}: {total} vs {want}");
                assert_eq!(polys.len(), if lon0 == 0.0 { 1 } else { 2 }, "lon0 {lon0}");
            }
        }
    }

    #[test]
    fn data_already_inside_is_left_alone() {
        let line = Geometry::Line(vec![vec![c(-170.0, 0.0), c(-100.0, 10.0), c(80.0, -5.0)]]);
        assert_eq!(cut(&line, -90.0), line);
        assert_eq!(cut(&Geometry::Point(c(120.0, 1.0)), -90.0), Geometry::Point(c(-240.0, 1.0)));
        // Under a 0° central meridian nothing moves, not even points on ±180°.
        let world =
            Geometry::Polygon(vec![vec![vec![c(-180.0, -60.0), c(180.0, -60.0), c(180.0, 60.0), c(-180.0, 60.0), c(-180.0, -60.0)]]]);
        assert_eq!(cut(&world, 0.0), world);
        for x in [-180.0, 180.0] {
            assert_eq!(cut(&Geometry::Point(c(x, 5.0)), 0.0), Geometry::Point(c(x, 5.0)), "{x} stays on its own edge");
            // So does a line lying along the edge (the graticule's meridian there).
            let meridian = Geometry::Line(vec![vec![c(x, -80.0), c(x, 0.0), c(x, 80.0)]]);
            assert_eq!(cut(&meridian, 0.0), meridian, "{x}");
        }
        // The same for a projection's east edge elsewhere: 90°E under 90°W.
        let east = Geometry::Line(vec![vec![c(90.0, -10.0), c(90.0, 10.0)]]);
        assert_eq!(cut(&east, -90.0), east);
    }

    #[test]
    fn hostile_coordinates_are_cut_or_left_alone_without_panicking() {
        // A missing coordinate breaks a line (as the renderer does) and is left out of a ring,
        // and the rest is still cut.
        let river = Geometry::Line(vec![vec![c(70.0, 50.0), c(f64::NAN, 0.0), c(80.0, 50.0), c(100.0, 52.0)]]);
        let Geometry::Line(parts) = cut(&river, -90.0) else { panic!("a line") };
        assert_eq!(parts.len(), 2, "the lone point is dropped, the run across the edge cut: {parts:?}");
        assert!(parts.iter().flatten().all(|c| c.is_finite() && (-270.0..=90.0).contains(&c.x)), "{parts:?}");
        let ring = vec![c(60.0, 40.0), c(120.0, 40.0), c(f64::INFINITY, 50.0), c(120.0, 60.0), c(60.0, 60.0), c(60.0, 40.0)];
        let Geometry::Polygon(polys) = cut(&Geometry::Polygon(vec![vec![ring]]), -90.0) else { panic!("a polygon") };
        assert_eq!(polys.len(), 2, "{polys:?}");
        // Longitudes no data has, and absurd central meridians, are left for the projection.
        for x in [-1e300, 1e300, f64::MIN, f64::MAX] {
            let line = Geometry::Line(vec![vec![c(x, 0.0), c(x, 1.0)]]);
            assert_eq!(cut(&line, -90.0), line, "{x}");
            let ring = Geometry::Polygon(vec![vec![vec![c(x, 0.0), c(x, 1.0), c(x * 0.5, 1.0), c(x, 0.0)]]]);
            assert_eq!(cut(&ring, 0.0), ring, "{x}");
            assert_eq!(cut(&Geometry::Point(c(x, 1.0)), 0.0), Geometry::Point(c(x, 1.0)), "{x}");
            let square = Geometry::Polygon(vec![vec![vec![c(60.0, 40.0), c(120.0, 40.0), c(120.0, 60.0), c(60.0, 40.0)]]]);
            assert_eq!(cut(&square, x), square, "lon0 {x}");
        }
        assert_eq!(cut(&Geometry::Line(vec![vec![]]), 0.0), Geometry::Line(vec![]));
        assert_eq!(cut(&Geometry::Polygon(vec![vec![vec![]]]), -90.0), Geometry::Polygon(vec![vec![vec![]]]));
    }

    #[test]
    fn any_margin_is_safe() {
        let river = Geometry::Line(vec![vec![c(80.0, 50.0), c(100.0, 52.0)]]);
        let square = Geometry::Polygon(vec![vec![vec![c(60.0, 40.0), c(120.0, 40.0), c(120.0, 60.0), c(60.0, 60.0), c(60.0, 40.0)]]]);
        let lake = Geometry::Polygon(vec![vec![vec![c(10.0, 40.0), c(20.0, 40.0), c(20.0, 50.0), c(10.0, 40.0)]]]);
        for m in [0.0, 1.0, 500.0, -5.0, f64::NAN, f64::INFINITY] {
            let Geometry::Line(parts) = cut_with_margin(&river, -90.0, &|_| m) else { panic!("a line") };
            assert_eq!(parts.len(), 2, "margin {m}: {parts:?}");
            assert!(parts.iter().flatten().all(|c| c.is_finite() && (-270.0..=90.0).contains(&c.x)), "margin {m}: {parts:?}");
            let Geometry::Polygon(polys) = cut_with_margin(&square, -90.0, &|_| m) else { panic!("a polygon") };
            assert_eq!(polys.len(), 2, "margin {m}: {polys:?}");
            assert!(polys.iter().flatten().flatten().all(|c| c.is_finite() && (-270.0..=90.0).contains(&c.x)), "margin {m}: {polys:?}");
            // Geometry that needs no cut stays finite and in the turn whatever the margin.
            let Geometry::Polygon(kept) = cut_with_margin(&lake, -90.0, &|_| m) else { panic!("a polygon") };
            assert!(kept.iter().flatten().flatten().all(|c| c.is_finite() && (-270.0..=90.0).contains(&c.x)), "margin {m}: {kept:?}");
        }
    }

    #[test]
    fn near_a_bent_edge_points_are_pulled_inside() {
        // Under a datum shift the edge strays by the margin: a point that needs no cut but lies
        // within it of the edge is pulled inside, the rest stay exactly where they are.
        let line = Geometry::Line(vec![vec![c(80.0, 60.0), c(89.995, 61.0)]]);
        let Geometry::Line(parts) = cut_with_margin(&line, -90.0, &|_| 0.01) else { panic!("a line") };
        assert_eq!(parts[0][0], c(80.0, 60.0));
        assert!((parts[0][1].x - (90.0 - 0.01)).abs() < 1e-6 && parts[0][1].x < 89.99, "{parts:?}");
        assert_eq!(cut_with_margin(&Geometry::Point(c(-269.995, 1.0)), -90.0, &|_| 0.01), Geometry::Point(c(-270.0 + 0.01 + EDGE, 1.0)));
        // Without a margin nothing moves, not even a point on the edge.
        assert_eq!(cut_with_margin(&line, -90.0, &|_| 0.0), line);
        assert_eq!(cut(&Geometry::Point(c(90.0, 1.0)), -90.0), Geometry::Point(c(90.0, 1.0)));
    }

    #[test]
    fn a_hole_finds_its_piece_from_any_point_off_the_cut() {
        let square = vec![vec![c(0.0, 0.0), c(10.0, 0.0), c(10.0, 10.0), c(0.0, 10.0), c(0.0, 0.0)]];
        // Two vertices on the cut (x = 0) and one on the exterior's own edge, where the
        // even-odd test can go either way: an edge midpoint places it.
        let hole = [c(0.0, 2.0), c(10.0, 5.0), c(0.0, 8.0), c(0.0, 2.0)];
        let boxes = |polys: &[Vec<Vec<Coord>>]| polys.iter().map(|p| bounds(&p[0])).collect::<Vec<_>>();
        let one = std::slice::from_ref(&square);
        assert_eq!(home(one, &boxes(one), &hole, 0.0, 360.0), Some(0));
        // A sliver lying just outside the exterior's edge (rounding at the cut): no point of it is
        // inside any piece, and it goes to the nearest piece rather than being dropped.
        let far = vec![vec![c(100.0, 0.0), c(110.0, 0.0), c(110.0, 10.0), c(100.0, 10.0), c(100.0, 0.0)]];
        let sliver = [c(10.0, 2.0), c(10.0, 8.0), c(10.0 + 1e-9, 5.0), c(10.0, 2.0)];
        let two = [far.clone(), square];
        assert_eq!(home(&two, &boxes(&two), &sliver, 0.0, 360.0), Some(1));
        let only = [far];
        assert_eq!(home(&only, &boxes(&only), &sliver, 0.0, 360.0), Some(0), "the only piece");
        assert_eq!(home(&[], &[], &sliver, 0.0, 360.0), None);
    }
}
