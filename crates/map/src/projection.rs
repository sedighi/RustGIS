//! Map projections: how the flat map lays the Earth out (the globe is in [`crate::globe`]).
//!
//! A map is drawn in Web Mercator by default, or in any projected coordinate system: a preset
//! (world and polar projections, regional equal-area grids) or an EPSG code. The camera then
//! works in that system's units, so panning, zooming and hit testing are the same for all of
//! them; only conversions between longitude/latitude and map coordinates go through here.

use rustgis_geom::{Coord, Extent, lonlat_to_merc, merc_to_lonlat};

/// A named projection: (id, label, PROJ definition).
pub const PRESETS: &[(&str, &str, &str)] = &[
    ("equirectangular", "Equirectangular (Plate Carrée)", "+proj=eqc +lat_ts=0 +lon_0=0 +datum=WGS84 +units=m +no_defs"),
    ("miller", "Miller Cylindrical", "+proj=mill +lon_0=0 +R_A +datum=WGS84 +units=m +no_defs"),
    ("mollweide", "Mollweide (equal-area world)", "+proj=moll +lon_0=0 +datum=WGS84 +units=m +no_defs"),
    ("cylindricalEqualArea", "Lambert Cylindrical Equal Area", "+proj=cea +lon_0=0 +lat_ts=0 +datum=WGS84 +units=m +no_defs"),
    ("robinson", "Robinson", "+proj=robin +lon_0=0 +datum=WGS84 +units=m +no_defs"),
    ("equalEarth", "Equal Earth", "+proj=eqearth +lon_0=0 +datum=WGS84 +units=m +no_defs"),
    ("naturalEarth", "Natural Earth", "+proj=natearth +lon_0=0 +datum=WGS84 +units=m +no_defs"),
    ("winkelTripel", "Winkel Tripel", "+proj=wintri +lon_0=0 +datum=WGS84 +units=m +no_defs"),
    ("eckertIV", "Eckert IV (equal-area world)", "+proj=eck4 +lon_0=0 +datum=WGS84 +units=m +no_defs"),
    ("northPolar", "North Polar Stereographic", "+proj=stere +lat_0=90 +lat_ts=70 +lon_0=0 +datum=WGS84 +units=m +no_defs"),
    ("southPolar", "South Polar Stereographic", "+proj=stere +lat_0=-90 +lat_ts=-71 +lon_0=0 +datum=WGS84 +units=m +no_defs"),
    (
        "europeEqualArea",
        "Europe Equal Area (ETRS89 LAEA)",
        "+proj=laea +lat_0=52 +lon_0=10 +x_0=4321000 +y_0=3210000 +ellps=GRS80 +units=m +no_defs",
    ),
    (
        "usaAlbers",
        "USA Contiguous Albers Equal Area",
        "+proj=aea +lat_0=23 +lon_0=-96 +lat_1=29.5 +lat_2=45.5 +datum=NAD83 +units=m +no_defs",
    ),
];

/// Whether local views (`local:lon,lat`, azimuthal equidistant) can be drawn in this build. They
/// need proj4rs' `aeqd`, which the browser build leaves out (Cargo.toml); asking the projection
/// itself, rather than the target, keeps the command, the ribbon and the tests in step with it.
pub fn local_views_supported() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(|| Projector::new("local:0,0").is_ok())
}

/// The PROJ definition and label of a projection code: a preset id or `EPSG:<code>`.
pub fn lookup(code: &str) -> Result<(String, String), String> {
    let code = code.trim();
    // `local:lon,lat`: an azimuthal equidistant projection centred on a point (a local scene).
    if let Some(rest) = code.strip_prefix("local:") {
        // Exactly two finite numbers; anything else is refused rather than half-read.
        let n: Option<Vec<f64>> = rest.split(',').map(|x| x.trim().parse::<f64>().ok().filter(|v| v.is_finite())).collect();
        let Some(&[lon, lat]) = n.as_deref() else { return Err("a local view is local:lon,lat".into()) };
        let (lon, lat) = ((lon + 180.0).rem_euclid(360.0) - 180.0, lat.clamp(-89.0, 89.0));
        return Ok((
            format!("+proj=aeqd +lat_0={lat} +lon_0={lon} +x_0=0 +y_0=0 +datum=WGS84 +units=m +no_defs"),
            format!("Local ({lon:.2}°, {lat:.2}°)"),
        ));
    }
    if let Some((_, label, def)) = PRESETS.iter().find(|(id, label, _)| id.eq_ignore_ascii_case(code) || label.eq_ignore_ascii_case(code)) {
        return Ok((def.to_string(), label.to_string()));
    }
    // `3035`, `EPSG:3035` or `epsg 3035`: an optional prefix, then only digits.
    let lower = code.to_ascii_lowercase();
    let digits = lower.strip_prefix("epsg").map(|r| r.trim_start_matches([':', ' '])).unwrap_or(&lower);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("unknown projection '{code}'"));
    }
    let unknown = || format!("unknown coordinate system EPSG:{}", digits.trim_start_matches('0'));
    let n = digits.parse::<u16>().map_err(|_| unknown())?;
    let def = rustgis_data::raster::crs::epsg_proj4(n).ok_or_else(unknown)?;
    if def.contains("+proj=longlat") {
        return Err(format!("EPSG:{n} is in degrees: pick a projected coordinate system"));
    }
    if def.contains("+proj=geocent") {
        return Err(format!("EPSG:{n} isn't a map projection"));
    }
    Ok((def.to_string(), format!("EPSG:{n}")))
}

/// Where a projection is meant to be used. Points outside are not drawn: far from it most
/// projections distort wildly, fold back or run off to infinity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Domain {
    /// The whole world (cylindrical and pseudo-cylindrical projections).
    World,
    /// A longitude/latitude box (regional presets).
    Box(Extent),
    /// Within `radius` degrees of a centre (azimuthal and stereographic projections).
    Cap { center: Coord, radius: f64 },
    /// Within `half` degrees of longitude of a central meridian (transverse Mercator, UTM).
    Band { lon0: f64, half: f64 },
}

impl Domain {
    /// The area of use for a regional preset, or one worked out from the PROJ parameters.
    /// `(lon0, lat0)` is the projection's centre (`lon0` east of Greenwich), as proj4rs reads it,
    /// and `pm` the prime meridian the definition's other longitudes (a UTM zone's) are counted from.
    fn of(id: Option<&str>, def: &str, (lon0, lat0): (f64, f64), pm: f64) -> Domain {
        match id {
            Some("europeEqualArea") => return Domain::Box(Extent::new(-35.0, 25.0, 50.0, 75.0)),
            Some("usaAlbers") => return Domain::Box(Extent::new(-130.0, 18.0, -60.0, 55.0)),
            _ => {}
        }
        let param = |k: &str| -> Option<f64> {
            def.split_whitespace().find_map(|t| t.strip_prefix(&format!("+{k}="))).and_then(|v| v.parse().ok())
        };
        let proj = def.split_whitespace().find_map(|t| t.strip_prefix("+proj=")).unwrap_or("");
        match proj {
            "tmerc" | "etmerc" | "utm" => {
                let lon0 = if proj == "utm" { param("zone").map(|z| z * 6.0 - 183.0 + pm).unwrap_or(lon0) } else { lon0 };
                Domain::Band { lon0, half: 30.0 }
            }
            "stere" | "sterea" | "somerc" | "krovak" => Domain::Cap { center: Coord::new(lon0, lat0), radius: 60.0 },
            "laea" | "aeqd" => Domain::Cap { center: Coord::new(lon0, lat0), radius: 120.0 },
            "aea" | "lcc" => {
                let (a, b) = (param("lat_1").unwrap_or(lat0), param("lat_2").unwrap_or(lat0));
                let (lo, hi) = (a.min(b) - 35.0, a.max(b) + 35.0);
                Domain::Box(Extent::new(lon0 - 90.0, lo.max(-89.0), lon0 + 90.0, hi.min(89.0)))
            }
            "geos" => Domain::Cap { center: Coord::new(lon0, 0.0), radius: 75.0 },
            _ => Domain::World,
        }
    }

    pub fn contains(&self, c: Coord) -> bool {
        match *self {
            Domain::World => true,
            // A box centred near the antimeridian (lon_0 = 150) runs past ±180°.
            Domain::Box(e) => [0.0, 360.0, -360.0].iter().any(|d| e.contains(Coord::new(c.x + d, c.y))),
            Domain::Cap { center, radius } => rustgis_geom::haversine_m(center, c) <= radius.to_radians() * rustgis_geom::EARTH_RADIUS_M,
            Domain::Band { lon0, half } => ((c.x - lon0 + 540.0).rem_euclid(360.0) - 180.0).abs() <= half && c.y.abs() <= 84.0,
        }
    }
}

/// The prime meridian of a definition (`+pm=`: a name such as `paris`, or degrees), in degrees
/// east of Greenwich: `lon_0` and the definition's other longitudes are counted from it. 0 when
/// there is none, or when it can't be read (proj4rs then refuses the definition anyway).
fn prime_meridian(def: &str) -> f64 {
    angle(def, "pm").unwrap_or(0.0)
}

/// A definition's `+lon_0`, `+lat_0` or `+pm` in degrees, read the way proj4rs reads it (degrees,
/// DMS, or a prime meridian's name), so the world projections computed here and the areas of use
/// agree with it. `None` when it is missing or can't be read.
fn angle(def: &str, key: &str) -> Option<f64> {
    let v = def.split_whitespace().find_map(|t| t.strip_prefix(&format!("+{key}=")))?;
    let p = proj4rs::Proj::from_proj_string(&format!("+proj=longlat +ellps=WGS84 +{key}={v}")).ok()?;
    let d = match key {
        "pm" => p.from_greenwich(),
        "lat_0" => p.latitude_of_origin(),
        _ => p.longitude_of_origin(),
    }
    .to_degrees();
    d.is_finite().then_some(d)
}

/// How a projection is computed: by the PROJ port, or here (world projections it lacks).
#[derive(Clone)]
enum Engine {
    Proj { native: Box<proj4rs::Proj>, wgs84: Box<proj4rs::Proj> },
    World(crate::world::World),
}

/// Converts between longitude/latitude and the coordinates of a custom projection.
#[derive(Clone)]
pub struct Projector {
    engine: Engine,
    /// Metres per map unit.
    to_meter: f64,
    domain: Domain,
    /// The central meridian east of Greenwich, for cutting geometry along the map's edge.
    lon0: f64,
    /// Where the map's edge runs in WGS84 longitudes, worked out on first use (see [`Projector::edge`]).
    edge: std::sync::OnceLock<Edge>,
}

/// The edge is sampled every `EDGE_STEP`° of latitude from 90°S to 90°N: `EDGE_SAMPLES` latitudes,
/// the equator's at `EQUATOR`.
const EDGE_STEP: f64 = 5.0;
const EDGE_SAMPLES: usize = 37;
const EQUATOR: usize = EDGE_SAMPLES / 2;

/// A world projection's edge in WGS84 longitudes: the meridian it runs along on average, and how far
/// it strays from it every 5° of latitude. A datum shift moves the edge off the meridian opposite
/// the central one (by hundreds of metres for a local datum) and bends it, more and more towards the
/// poles, where a shift in metres spans more longitude.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    /// Not normalised to ±180°: 330 for a projection centred on 150°E.
    pub lon: f64,
    /// How far the edge strays from `lon`, east or west (an absolute distance in degrees), at
    /// latitudes -90, -85, … 90 (±89.9 at the poles).
    strays: [f64; EDGE_SAMPLES],
}

impl Edge {
    /// How far the edge may stray at a latitude: the larger of the two samples around it (it
    /// grows steadily towards the poles).
    pub fn stray_at(&self, lat: f64) -> f64 {
        let i = ((lat.clamp(-90.0, 90.0) + 90.0) / EDGE_STEP).floor() as usize;
        let (a, b) = (self.strays.get(i).copied().unwrap_or(0.0), self.strays.get(i + 1).copied().unwrap_or(0.0));
        a.max(b)
    }
}

/// The edge's stray at every sampled latitude, from the ones `measured`. A latitude the projection
/// can't show (`shown` false: Mercator's poles) needs no margin. One it can show but where the edge
/// wasn't found gets its nearest measured neighbours' stray scaled to it (a shift in metres spans
/// longitude as 1 / cos latitude), the larger of the two, with half again to spare, rather than
/// the much wider stray found nearer a pole.
fn fill_strays(measured: &[Option<f64>], lats: &[f64], shown: &[bool]) -> [f64; EDGE_SAMPLES] {
    let scaled = |v: f64, from: f64, to: f64| v * from.to_radians().cos() / to.to_radians().cos().max(1e-3);
    let nearest = |i: usize, step: isize| -> Option<f64> {
        let mut j = i as isize + step;
        while let Ok(k) = usize::try_from(j) {
            let m = measured.get(k)?;
            if let (Some(v), Some(from), Some(to)) = (m, lats.get(k), lats.get(i)) {
                return Some(scaled(*v, *from, *to));
            }
            j += step;
        }
        None
    };
    let mut strays = [0.0; EDGE_SAMPLES];
    for (i, s) in strays.iter_mut().enumerate() {
        *s = match measured.get(i).copied().flatten() {
            Some(v) => v,
            None if shown.get(i).copied().unwrap_or(false) => 1.5 * nearest(i, -1).unwrap_or(0.0).max(nearest(i, 1).unwrap_or(0.0)),
            None => 0.0,
        };
    }
    strays
}

impl std::fmt::Debug for Projector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match &self.engine {
            Engine::Proj { native, .. } => native.projname(),
            Engine::World(w) => w.kind.proj_name(),
        };
        write!(f, "Projector({name})")
    }
}

/// Map coordinates beyond this are treated as unprojectable (the projection's edge).
const LIMIT: f64 = 4.0e7;

impl Projector {
    /// The projector for a code `lookup` accepts: a preset id or name, `EPSG:<n>` or
    /// `local:lon,lat`. World projections proj4rs lacks are computed here; the rest go to
    /// proj4rs. Fails when the code is unknown, or when its method can't be computed (its
    /// `+proj=` name is in the message), so a map is never drawn in another projection than the
    /// one it names.
    pub fn new(code: &str) -> Result<Projector, String> {
        let (def, _) = lookup(code)?;
        let code_t = code.trim();
        let preset = PRESETS
            .iter()
            .find(|(id, label, _)| id.eq_ignore_ascii_case(code_t) || label.eq_ignore_ascii_case(code_t))
            .map(|(id, _, _)| *id);
        let proj = def.split_whitespace().find_map(|t| t.strip_prefix("+proj=")).unwrap_or("");
        if let Some(kind) = crate::world::Kind::from_proj_name(proj) {
            // Computed here: the centre is read from the definition as proj4rs would read it.
            let pm = prime_meridian(&def);
            let lon0 = angle(&def, "lon_0").unwrap_or(0.0) + pm;
            let lat0 = angle(&def, "lat_0").unwrap_or(0.0);
            let domain = Domain::of(preset, &def, (lon0, lat0), pm);
            let engine = Engine::World(crate::world::World { kind, lon0 });
            return Ok(Projector { engine, to_meter: 1.0, domain, lon0, edge: std::sync::OnceLock::new() });
        }
        let native = proj4rs::Proj::from_proj_string(&def).map_err(|e| match e {
            // A known code whose method isn't implemented (EPSG:5880's +proj=poly, say): name it.
            proj4rs::errors::Error::ProjectionNotFound => format!("{code}: its projection method (+proj={proj}) isn't supported"),
            e => format!("{code}: {e}"),
        })?;
        let wgs84 = proj4rs::Proj::from_proj_string("+proj=longlat +datum=WGS84 +no_defs").map_err(|e| e.to_string())?;
        let to_meter = native.to_meter();
        // The prime and central meridians proj4rs itself projects with, so the area of use, the
        // edge and the projection all agree (it reads degrees, DMS, radians and names alike).
        let finite = |d: f64| if d.is_finite() { d } else { 0.0 };
        let pm = finite(native.from_greenwich().to_degrees());
        let lon0 = finite(native.longitude_of_origin().to_degrees()) + pm;
        let lat0 = finite(native.latitude_of_origin().to_degrees());
        let domain = Domain::of(preset, &def, (lon0, lat0), pm);
        let engine = Engine::Proj { native: Box::new(native), wgs84: Box::new(wgs84) };
        let to_meter = if to_meter > 0.0 { to_meter } else { 1.0 };
        Ok(Projector { engine, to_meter, domain, lon0, edge: std::sync::OnceLock::new() })
    }

    /// The central meridian, in WGS84 longitudes, of a projection that shows the whole world: the
    /// meridian opposite its edge (`edge().lon - 180`), which a datum shift moves slightly off
    /// `lon_0` (plus `+pm`). `None` for one limited to an area of use, which never reaches it.
    pub fn central_meridian(&self) -> Option<f64> {
        self.edge().map(|e| e.lon - 180.0)
    }

    /// Where the edge of a projection that shows the whole world runs, in WGS84 longitudes, worked
    /// out once; `None` for one limited to an area of use.
    ///
    /// Without a datum shift it is the meridian opposite the central one: a probe at the equator
    /// and at 60°N and S finds the map jumping sides exactly there. With one, it is found every 5°
    /// of latitude by bisecting for where the map jumps from one side to the other. If even the
    /// equator's edge can't be found, it falls back to the nominal edge with no margin;
    /// `every_world_projection_wraps_opposite_its_central_meridian` checks that no EPSG world
    /// projection does.
    pub fn edge(&self) -> Option<&Edge> {
        if self.domain != Domain::World {
            return None;
        }
        Some(self.edge.get_or_init(|| {
            let exact = Edge { lon: self.lon0 + 180.0, strays: [0.0; EDGE_SAMPLES] };
            let Engine::Proj { .. } = self.engine else { return exact };
            let shifted = [0.0, 60.0, -60.0].iter().any(|lat| self.edge_at(*lat, exact.lon).is_none_or(|x| (x - exact.lon).abs() > 1e-9));
            if shifted { self.find_edge(exact.lon).unwrap_or(exact) } else { exact }
        }))
    }

    fn find_edge(&self, guess: f64) -> Option<Edge> {
        let lats: Vec<f64> = (0..EDGE_SAMPLES).map(|i| (-90.0 + EDGE_STEP * i as f64).clamp(-89.9, 89.9)).collect();
        let found: Vec<Option<f64>> = lats.iter().map(|lat| self.edge_at(*lat, guess)).collect();
        // The equator's edge: most of the map, and where the edge strays least.
        let lon = (*found.get(EQUATOR)?)?;
        let measured: Vec<Option<f64>> = found.iter().map(|f| f.map(|x| (x - lon).abs())).collect();
        let shown: Vec<bool> = lats.iter().map(|lat| self.forward(Coord::new(guess - 180.0, *lat)).is_some()).collect();
        Some(Edge { lon, strays: fill_strays(&measured, &lats, &shown) })
    }

    /// The WGS84 longitude where the map jumps sides at a latitude, near `guess`.
    fn edge_at(&self, lat: f64, guess: f64) -> Option<f64> {
        let centre = self.forward(Coord::new(guess - 180.0, lat))?.x;
        // West of the edge a point draws on the map's east side, past it on the west side.
        let east_side = |lon: f64| self.forward(Coord::new(lon, lat)).map(|m| m.x > centre);
        // Near the poles a shift of a few hundred metres spans a degree or more: widen the search
        // until it holds the jump.
        let (mut a, mut b) = [10.0, 30.0, 90.0]
            .into_iter()
            .map(|h| (guess - h, guess + h))
            .find(|&(a, b)| east_side(a) == Some(true) && east_side(b) == Some(false))?;
        for _ in 0..60 {
            let m = (a + b) / 2.0;
            if east_side(m)? {
                a = m;
            } else {
                b = m;
            }
        }
        Some((a + b) / 2.0)
    }

    /// Longitude/latitude to map coordinates, `None` outside the projection.
    pub fn forward(&self, c: Coord) -> Option<Coord> {
        if !c.is_finite() {
            return None;
        }
        let (native, wgs84) = match &self.engine {
            Engine::Proj { native, wgs84 } => (native, wgs84),
            Engine::World(w) => return w.forward(c),
        };
        let mut p = (c.x.to_radians(), c.y.clamp(-90.0, 90.0).to_radians(), 0.0);
        proj4rs::transform::transform(wgs84, native, &mut p).ok()?;
        (p.0.is_finite() && p.1.is_finite() && p.0.abs() < LIMIT && p.1.abs() < LIMIT).then(|| Coord::new(p.0, p.1))
    }

    /// Like [`Projector::forward`], but also rejects points that don't map back to themselves
    /// (wrapped or folded parts of a projection, such as the far hemisphere of a polar one).
    pub fn forward_checked(&self, c: Coord) -> Option<Coord> {
        if !self.domain.contains(c) {
            return None;
        }
        let m = self.forward(c)?;
        // World projections don't fold, and on their edges (±180°) the inverse is too imprecise
        // to compare: only projections limited to an area of use need the round trip.
        if self.domain == Domain::World {
            return Some(m);
        }
        let back = self.inverse(m)?;
        let dlon = ((back.x - c.x + 540.0).rem_euclid(360.0) - 180.0).abs();
        // At a pole every longitude is the same point.
        let same = (dlon < 1e-3 && (back.y - c.y).abs() < 1e-3) || (c.y.abs() > 89.99 && (back.y - c.y).abs() < 1e-3);
        same.then_some(m)
    }

    /// Map coordinates to longitude/latitude.
    pub fn inverse(&self, m: Coord) -> Option<Coord> {
        if !m.is_finite() {
            return None;
        }
        let (native, wgs84) = match &self.engine {
            Engine::Proj { native, wgs84 } => (native, wgs84),
            Engine::World(w) => return w.inverse(m),
        };
        let mut p = (m.x, m.y, 0.0);
        proj4rs::transform::transform(native, wgs84, &mut p).ok()?;
        let c = Coord::new(p.0.to_degrees(), p.1.to_degrees());
        (c.is_finite() && c.y.abs() <= 90.0 + 1e-9).then_some(c)
    }

    pub fn to_meter(&self) -> f64 {
        self.to_meter
    }

    /// Ground metres per map unit at a map point, measured: a 1 km step east-west (the scale
    /// bar's direction; north-south on the projection's edge) compared with the great-circle
    /// distance it covers, on the sphere Web Mercator's scale uses so both agree. Falls back to
    /// the unit size where no step stays in the projection.
    pub fn ground_per_unit(&self, at: Coord) -> f64 {
        let Some(c) = self.inverse(at) else { return self.to_meter };
        let step = 1000.0 / self.to_meter;
        let radius_ratio = rustgis_geom::MERCATOR_R / rustgis_geom::EARTH_RADIUS_M;
        let steps = [Coord::new(step, 0.0), Coord::new(-step, 0.0), Coord::new(0.0, step), Coord::new(0.0, -step)];
        steps
            .iter()
            .filter_map(|s| self.inverse(Coord::new(at.x + s.x, at.y + s.y)))
            .map(|d| rustgis_geom::haversine_m(c, d) * radius_ratio / step)
            .find(|k| k.is_finite() && *k > 0.0)
            .unwrap_or(self.to_meter)
    }

    pub fn domain(&self) -> Domain {
        self.domain
    }
}

/// The flat map's projection, ready to convert coordinates.
#[derive(Clone, Debug)]
pub enum Flat {
    /// Web Mercator (also the camera space of the globe).
    Mercator,
    Custom(std::sync::Arc<Projector>),
}

impl Flat {
    /// Longitude/latitude to map coordinates.
    pub fn forward(&self, c: Coord) -> Option<Coord> {
        match self {
            Flat::Mercator => c.is_finite().then(|| lonlat_to_merc(c)),
            Flat::Custom(p) => p.forward(c),
        }
    }

    /// For geometry and other bulk conversion: drops points a projection folds or wraps.
    pub fn forward_checked(&self, c: Coord) -> Option<Coord> {
        match self {
            Flat::Mercator => self.forward(c),
            Flat::Custom(p) => p.forward_checked(c),
        }
    }

    /// A longitude/latitude geometry in map coordinates, for drawing. Under a world projection it
    /// is first cut along the map's edge, so a feature across it (Russia under a 90°W central
    /// meridian) is drawn on both sides rather than as a band across the map (#209). Points the
    /// projection can't show become NaN, which the renderer skips.
    pub fn project(&self, g: &rustgis_data::Geometry) -> rustgis_data::Geometry {
        let f = |c: Coord| self.forward_checked(c).unwrap_or(Coord::new(f64::NAN, f64::NAN));
        if let Flat::Custom(p) = self
            && let Some(edge) = p.edge()
        {
            return crate::antimeridian::cut_with_margin(g, edge.lon - 180.0, &|lat| edge.stray_at(lat)).map_coords(&f);
        }
        g.map_coords(&f)
    }

    /// Map coordinates to longitude/latitude.
    pub fn inverse(&self, m: Coord) -> Option<Coord> {
        match self {
            Flat::Mercator => m.is_finite().then(|| merc_to_lonlat(m)),
            Flat::Custom(p) => p.inverse(m),
        }
    }

    /// Ground metres per map unit at a map point (Mercator stretches with latitude).
    pub fn ground_per_unit(&self, at: Coord) -> f64 {
        match self {
            Flat::Mercator => merc_to_lonlat(at).y.to_radians().cos().abs().max(0.01),
            Flat::Custom(p) => p.ground_per_unit(at),
        }
    }

    /// The map-coordinate box around a longitude/latitude extent (from points along its edges and
    /// inside it, so curved projections are covered).
    pub fn extent(&self, e: &Extent) -> Extent {
        let mut out = Extent::EMPTY;
        if e.is_empty() {
            return out;
        }
        let n = 16;
        for i in 0..=n {
            for j in 0..=n {
                let c = Coord::new(e.min_x + e.width() * i as f64 / n as f64, e.min_y + e.height() * j as f64 / n as f64);
                if let Some(m) = self.forward_checked(c) {
                    out.expand(m);
                }
            }
        }
        out
    }
}

thread_local! {
    static PROJECTORS: std::cell::RefCell<std::collections::HashMap<String, Option<std::sync::Arc<Projector>>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

impl Flat {
    /// The flat projection behind a map projection (the globe's camera space is Web Mercator; an
    /// unusable custom code falls back to it). Parsed projections are kept per thread.
    pub fn of(p: &crate::Projection) -> Flat {
        let crate::Projection::Custom(code) = p else { return Flat::Mercator };
        let key = code.trim().to_ascii_lowercase();
        PROJECTORS.with(|cell| {
            let mut map = cell.borrow_mut();
            // Codes come from project files and scripts: keep the cache bounded.
            if map.len() >= 32 && !map.contains_key(&key) {
                map.clear();
            }
            let entry = map.entry(key).or_insert_with(|| Projector::new(code).ok().map(std::sync::Arc::new));
            entry.clone().map(Flat::Custom).unwrap_or(Flat::Mercator)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_codes_are_read_strictly() {
        assert!(lookup("local:12.5,41.9").is_ok());
        for bad in ["local:a,10,20", "local:10", "local:10,20,30", "local:10,inf", "local:"] {
            assert!(lookup(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn presets_and_epsg_codes() {
        for (id, _, _) in PRESETS {
            let p = Projector::new(id).unwrap_or_else(|e| panic!("{id}: {e}"));
            let c = Coord::new(10.0, if id.contains("south") { -75.0 } else { 50.0 });
            let m = p.forward(c).unwrap_or_else(|| panic!("{id}"));
            let back = p.inverse(m).unwrap();
            assert!((back.x - c.x).abs() < 1e-6 && (back.y - c.y).abs() < 1e-6, "{id}: {back:?}");
        }
        assert!(Projector::new("EPSG:3035").is_ok());
        assert!(Projector::new("epsg:32633").is_ok());
        assert!(Projector::new("3857").is_ok());
        assert!(Projector::new("EPSG:4326").is_err(), "degrees are not a map projection");
        assert!(Projector::new("EPSG:1").is_err());
        assert!(Projector::new("bogus").is_err());
        assert!(Projector::new("12x").is_err());
        assert!(Projector::new("epsgfoo32617").is_err());
        // A conic centred on the Pacific keeps points across the antimeridian.
        let pacific = Domain::of(None, "+proj=lcc +lat_1=30 +lat_2=60 +lon_0=150", (150.0, 0.0), 0.0);
        assert!(pacific.contains(Coord::new(-170.0, 45.0)) && pacific.contains(Coord::new(120.0, 45.0)));
        assert!(!pacific.contains(Coord::new(0.0, 45.0)));
        assert!(Projector::new("EPSG 32617").is_ok());
        assert!(lookup("EPSG:99999").unwrap_err().contains("coordinate system"), "out of range is an unknown code");
        // Equal Earth by its EPSG code goes to the built-in engine too.
        let ee = Projector::new("EPSG:8857").unwrap();
        assert!(format!("{ee:?}").contains("eqearth"));
        // Codes crs-definitions lacks (EPSG 9020–20003) come from the data crate's own table
        // (opengeos/RustGIS#198): MAGNA-SIRGAS 2018 / Origen-Nacional, SIRGAS-Chile 2016 / UTM 19S.
        let (bogota, santiago) = (Coord::new(-74.08, 4.65), Coord::new(-70.65, -33.45));
        for (ok, proj, city) in [("EPSG:9377", "tmerc", bogota), ("epsg:9155", "utm", santiago)] {
            let (def, label) = lookup(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
            assert!(def.contains(&format!("+proj={proj}")) && label == ok.to_ascii_uppercase(), "{ok}: {def} {label}");
            let p = Projector::new(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
            let back = p.inverse(p.forward(city).unwrap()).unwrap();
            assert!((back.x - city.x).abs() < 1e-7 && (back.y - city.y).abs() < 1e-7, "{ok}: {back:?}");
        }
        assert!(lookup("EPSG:9148").unwrap_err().contains("degrees"), "SIRGAS-Chile 2013 is geographic");
    }

    #[test]
    fn codes_whose_method_cannot_be_drawn_are_refused() {
        // EPSG:5880 (SIRGAS 2000 / Brazil Polyconic) is in the EPSG table, but its method
        // (+proj=poly) has no implementation: refuse it rather than accept it and draw the map
        // in Web Mercator while naming EPSG:5880 (opengeos/RustGIS#208).
        assert!(lookup("EPSG:5880").is_ok(), "the code itself is known");
        let e = Projector::new("EPSG:5880").unwrap_err();
        assert!(e.contains("EPSG:5880") && e.contains("poly"), "{e}");
        let e = crate::Projection::from_code("EPSG:5880").unwrap_err();
        assert!(e.contains("poly"), "{e}");
        // Codes that can be drawn are still accepted, by number or preset.
        for ok in ["EPSG:3035", "epsg:32633", "EPSG:8858", "equalEarth", "usaAlbers", "mercator", "globe"] {
            crate::Projection::from_code(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        // Local views are accepted exactly where they can be drawn (`aeqd` is native-only, Cargo.toml).
        let local = crate::Projection::from_code("local:12.5,41.9");
        assert_eq!(local.is_ok(), local_views_supported(), "{local:?}");
        // Pinned to the target on purpose, so the probe can't drift from the Cargo.toml split unnoticed.
        assert_eq!(local_views_supported(), !cfg!(target_arch = "wasm32"), "aeqd on native targets only");
    }

    #[test]
    fn local_views_are_azimuthal_equidistant() {
        if !local_views_supported() {
            return; // the browser build (see codes_whose_method_cannot_be_drawn_are_refused)
        }
        // map.viewMode "local" relies on +proj=aeqd: it must build (not fall back to Web Mercator)
        // and keep true distances from its centre.
        let p = Projector::new("local:12.5,41.9").unwrap_or_else(|e| panic!("{e}"));
        let o = p.forward(Coord::new(12.5, 41.9)).unwrap();
        assert!(o.x.abs() < 1e-6 && o.y.abs() < 1e-6, "the centre is the origin: {o:?}");
        let c = Coord::new(13.5, 42.4);
        let m = p.forward(c).unwrap();
        let back = p.inverse(m).unwrap();
        assert!((back.x - c.x).abs() < 1e-7 && (back.y - c.y).abs() < 1e-7, "{back:?}");
        // Distance from the centre on the map is the WGS84 geodesic distance (Vincenty: 99 580.4 m).
        let d = (m.x * m.x + m.y * m.y).sqrt();
        assert!((d - 99_580.4).abs() < 1.0, "{d}");
    }

    #[test]
    fn folded_points_are_rejected_and_extents_project() {
        let north = Projector::new("northPolar").unwrap();
        assert!(north.forward_checked(Coord::new(0.0, 60.0)).is_some());
        assert!(north.forward_checked(Coord::new(0.0, -80.0)).is_none(), "the far pole");
        assert!(north.forward_checked(Coord::new(120.0, -5.0)).is_none(), "the southern hemisphere");
        let utm = Projector::new("EPSG:32633").unwrap();
        assert_eq!(utm.domain(), Domain::Band { lon0: 15.0, half: 30.0 });
        assert!(utm.forward_checked(Coord::new(15.0, 50.0)).is_some() && utm.forward_checked(Coord::new(-100.0, 40.0)).is_none());
        let usa = Projector::new("usaAlbers").unwrap();
        assert!(usa.forward_checked(Coord::new(-95.0, 38.0)).is_some() && usa.forward_checked(Coord::new(100.0, 30.0)).is_none());
        let moll = Flat::Custom(std::sync::Arc::new(Projector::new("mollweide").unwrap()));
        let e = moll.extent(&Extent::new(-180.0, -90.0, 180.0, 90.0));
        assert!(e.width() > 3.55e7 && e.height() > 1.75e7, "the whole world, edges included: {e:?}");
        // Mollweide's parallels are true at about ±40.7° and squeezed (0.9003) at the equator:
        // a map unit there covers 1/0.9003 m of ground.
        assert!((moll.ground_per_unit(Coord::new(0.0, 0.0)) - 1.0 / 0.9003).abs() < 2e-3);
        let at = moll.forward(Coord::new(10.0, 40.73)).unwrap();
        assert!((moll.ground_per_unit(at) - 1.0).abs() < 5e-3);
        // A preset by label gets the same area of use as by id.
        assert_eq!(Projector::new("USA Contiguous Albers Equal Area").unwrap().domain(), Projector::new("usaAlbers").unwrap().domain());
        assert!(Flat::Mercator.inverse(Coord::new(f64::NAN, 0.0)).is_none());
    }

    #[test]
    fn longitudes_are_counted_from_the_prime_meridian() {
        // EPSG:5329: Mercator with lon_0 = 3.19° east of Jakarta (106.81°E), so centred on 110°E,
        // where its false easting puts x = 3 900 000 m (give or take the datum shift).
        let merc = Projector::new("EPSG:5329").unwrap();
        let seam = merc.central_meridian().unwrap();
        // Its edge is nominally at 70°W, opposite 110°E; its datum shift (Bessel, towgs84
        // -403,684,41) moves it about 290 m from there in WGS84 longitudes.
        assert!((seam - 110.0).abs() < 0.01, "{seam}");
        let edge = merc.edge().unwrap();
        assert!(edge.stray_at(0.0) < 1e-4 && edge.stray_at(45.0) < 0.005, "{edge:?}");
        // Towards the poles the same shift spans more longitude.
        assert!(edge.stray_at(87.0) > edge.stray_at(45.0), "{edge:?}");
        // A projection on WGS84 has its edge exactly opposite its central meridian.
        let pdc = Projector::new("EPSG:3832").unwrap();
        let pdc = pdc.edge().unwrap();
        assert_eq!(pdc.lon, 330.0, "no datum shift: the nominal edge, exactly, without a search");
        assert!(pdc.strays.iter().all(|s| *s == 0.0), "{pdc:?}");
        let centre = merc.forward(Coord::new(110.0, 0.0)).unwrap();
        assert!((centre.x - 3_900_000.0).abs() < 2_000.0, "{centre:?}");
        // EPSG:31251: Gauss–Krüger with lon_0 = 28° east of Ferro (17.67°W), so its band is around
        // 10.33°E, not 28°E: Ireland is in it, the Caspian isn't.
        let gk = Projector::new("EPSG:31251").unwrap();
        let Domain::Band { lon0, half } = gk.domain() else { panic!("{:?}", gk.domain()) };
        assert!((lon0 - (28.0 - 17.666666666667)).abs() < 1e-6 && half == 30.0, "{lon0}");
        assert!(gk.forward_checked(Coord::new(13.0, 47.5)).is_some(), "Austria");
        assert!(gk.forward_checked(Coord::new(-8.0, 53.0)).is_some(), "Ireland");
        assert!(gk.forward_checked(Coord::new(50.0, 42.0)).is_none(), "the Caspian");
        // Names, degrees and nothing at all.
        assert!((prime_meridian("+proj=lcc +pm=paris") - 2.337229166667).abs() < 1e-9);
        assert!((prime_meridian("+proj=merc +pm=-17.5") + 17.5).abs() < 1e-9);
        assert_eq!(prime_meridian("+proj=merc +lon_0=10"), 0.0);
        assert_eq!(prime_meridian("+proj=merc +pm=nowhere"), 0.0);
        // lon_0 in DMS, read as proj4rs reads it, for the world projections computed here too.
        assert!(angle("+proj=eqearth +lon_0=90d30'W", "lon_0").is_some_and(|d| (d + 90.5).abs() < 1e-9));
        assert_eq!(angle("+proj=eqearth", "lon_0"), None);
        assert!(angle("+proj=stere +lat_0=-90 +lon_0=0", "lat_0").is_some_and(|d| (d + 90.0).abs() < 1e-9));
        assert!(angle("+proj=laea +lat_0=52d30'N", "lat_0").is_some_and(|d| (d - 52.5).abs() < 1e-9));
    }

    #[test]
    fn every_world_projection_wraps_opposite_its_central_meridian() {
        // For every EPSG code drawn as a whole-world map, the map jumps sides at `central_meridian() + 180°`:
        // just west of it draws on the east, just east of it on the west.
        let mut checked = 0;
        for n in 1024..=32767u32 {
            // Codes with a prime meridian other than Greenwich (+pm) and a datum shift included.
            let Ok(p) = Projector::new(&format!("EPSG:{n}")) else { continue };
            let Some(lon0) = p.central_meridian() else { continue };
            let x = |lon: f64| p.forward(Coord::new(lon, 0.0)).map(|m| m.x);
            let (Some(west), Some(centre), Some(east)) = (x(lon0 + 179.5), x(lon0), x(lon0 + 180.5)) else { continue };
            assert!(west > centre && centre > east, "EPSG:{n}: lon_0 {lon0}: {west} / {centre} / {east}");
            // With a datum shift the edge is found by search, which must not fall back to the
            // nominal edge (no margin) for any of them.
            if let Engine::Proj { .. } = p.engine {
                assert!(p.find_edge(p.lon0 + 180.0).is_some(), "EPSG:{n}: the edge search fell back");
            }
            checked += 1;
        }
        // 31 in crs-definitions 0.6 (Mercator, equirectangular, cylindrical equal-area, Equal Earth),
        // 6 of them Mercator from Jakarta (+pm).
        assert!(checked >= 31, "only {checked} world projections checked");
    }

    #[test]
    fn unfound_edges_get_their_neighbours_stray() {
        let lats: Vec<f64> = (0..EDGE_SAMPLES).map(|i| (-90.0 + EDGE_STEP * i as f64).clamp(-89.9, 89.9)).collect();
        // A shift spanning 0.001° at the equator, as 1 / cos latitude.
        let mut measured: Vec<Option<f64>> = lats.iter().map(|l| Some(0.001 / l.to_radians().cos())).collect();
        let shown = vec![true; EDGE_SAMPLES];
        let at = |lat: f64| ((lat + 90.0) / EDGE_STEP) as usize;
        // Not found at 60°N, though shown: about the true 0.002°, with half again to spare, not the
        // pole's 0.57°.
        measured[at(60.0)] = None;
        let strays = fill_strays(&measured, &lats, &shown);
        assert!((strays[at(60.0)] - 1.5 * 0.002).abs() < 1e-4, "{}", strays[at(60.0)]);
        assert!(strays[at(60.0)] < strays[EDGE_SAMPLES - 1] / 10.0);
        // Not shown (Mercator's poles): no margin. Not found next to an unshown one: its other side.
        measured[EDGE_SAMPLES - 1] = None;
        measured[EDGE_SAMPLES - 2] = None;
        let mut shown = shown;
        shown[EDGE_SAMPLES - 1] = false;
        let strays = fill_strays(&measured, &lats, &shown);
        assert_eq!(strays[EDGE_SAMPLES - 1], 0.0);
        let expected = 1.5 * 0.001 / 85.0_f64.to_radians().cos(); // 80°N's, scaled to 85°N
        assert!((strays[EDGE_SAMPLES - 2] - expected).abs() / expected < 0.01, "{}", strays[EDGE_SAMPLES - 2]);
    }

    #[test]
    fn features_across_the_map_edge_are_cut_there() {
        use rustgis_data::Geometry;
        // #209: Equal Earth Americas (computed here, edge at 90°E) and PDC Mercator (proj4rs,
        // lon_0 = 150°, edge at 30°W). Before, these drew as bands across the whole map.
        // EPSG:5329 is centred on 110°E through its prime meridian (+pm=jakarta), with a datum shift.
        for (code, edge) in [("EPSG:8858", 90.0), ("EPSG:3832", -30.0), ("EPSG:5329", -70.0)] {
            let flat = Flat::of(&crate::Projection::Custom(code.into()));
            assert!(matches!(flat, Flat::Custom(_)), "{code}");
            let width = flat.extent(&Extent::new(-180.0, -80.0, 180.0, 80.0)).width();
            let line = Geometry::Line(vec![vec![Coord::new(edge - 10.0, 50.0), Coord::new(edge + 10.0, 52.0)]]);
            let square = Geometry::Polygon(vec![vec![vec![
                Coord::new(edge - 10.0, 40.0),
                Coord::new(edge + 10.0, 40.0),
                Coord::new(edge + 10.0, 60.0),
                Coord::new(edge - 10.0, 60.0),
                Coord::new(edge - 10.0, 40.0),
            ]]]);
            for g in [line, square] {
                let parts: Vec<Vec<Coord>> = match flat.project(&g) {
                    Geometry::Line(parts) => parts,
                    Geometry::Polygon(polys) => polys.into_iter().flatten().collect(),
                    other => panic!("{other:?}"),
                };
                assert_eq!(parts.len(), 2, "{code}: one piece on each side: {parts:?}");
                for part in &parts {
                    assert!(part.iter().all(|c| c.is_finite()), "{code}: {part:?}");
                    let jump = part.windows(2).map(|w| (w[1].x - w[0].x).abs()).fold(0.0, f64::max);
                    assert!(jump < width / 4.0, "{code}: no edge across the map ({jump} of {width})");
                }
                let side = |p: &Vec<Coord>| p.iter().map(|c| c.x).sum::<f64>().signum();
                assert_ne!(side(&parts[0]), side(&parts[1]), "{code}: the pieces sit on opposite edges");
            }
            // A hole across the edge is cut with its polygon: each side keeps its part of it.
            let ring = |x0: f64, x1: f64, y0: f64, y1: f64| {
                vec![Coord::new(x0, y0), Coord::new(x1, y0), Coord::new(x1, y1), Coord::new(x0, y1), Coord::new(x0, y0)]
            };
            let holed = Geometry::Polygon(vec![vec![ring(edge - 10.0, edge + 10.0, 40.0, 60.0), ring(edge - 5.0, edge + 5.0, 45.0, 55.0)]]);
            let Geometry::Polygon(polys) = flat.project(&holed) else { panic!("{code}: a polygon") };
            assert_eq!(polys.len(), 2, "{code}: one piece on each side: {polys:?}");
            for p in &polys {
                assert_eq!(p.len(), 2, "{code}: each piece keeps its part of the hole: {p:?}");
                assert!(p.iter().flatten().all(|c| c.is_finite()), "{code}: {p:?}");
            }
            let side = |p: &Vec<Vec<Coord>>| p[1].iter().map(|c| c.x).sum::<f64>().signum();
            assert_ne!(side(&polys[0]), side(&polys[1]), "{code}: the hole's parts sit on opposite edges");
        }
        // Limited projections never reach the far meridian, so nothing is cut.
        assert_eq!(Projector::new("EPSG:32633").unwrap().central_meridian(), None);
        assert_eq!(Projector::new("equalEarth").unwrap().central_meridian(), Some(0.0));
        assert!(
            Projector::new("EPSG:3832").unwrap().central_meridian().is_some_and(|s| (s - 150.0).abs() < 1e-9),
            "PDC Mercator: lon_0 = 150"
        );
    }
}
