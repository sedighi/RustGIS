//! EPSG codes 9020–20003, which `crs-definitions` lacks (opengeos/RustGIS#198).
//!
//! `crs-definitions` is generated from PostGIS's `spatial_ref_sys`, whose EPSG entries jump from
//! 9019 to 20004; the EPSG registry has added some 400 coordinate systems in between since
//! (SIRGAS-Chile, MAGNA-SIRGAS 2018 / Origen-Nacional, KSA-GRF17, NATRF2022 …). This table fills
//! the gap, and goes when an upstream release includes them: [`proj4`] is consulted only after
//! `crs_definitions::from_code` returns `None`, and a test fails for any entry upstream gains.
//!
//! Generated from PROJ 9.8.1's database (EPSG dataset v12.029, 2025-10-02) with
//! pyproj 3.8.0: every geographic 2D or projected system in the range that is not
//! deprecated, as `CRS.from_epsg(code).to_proj4()` without `+type=crs`, and whose projection
//! proj4rs implements (EPSG:10622, an orthographic, is left out). Geographic 3D, geocentric,
//! vertical and compound systems have no place in a raster's georeferencing.
//!
//! Datum shifts: where the first grid-free operation to WGS 84 that PROJ lists
//! (`TransformerGroup(crs, "EPSG:4326").transformers`) is a single non-zero Helmert step, it is
//! written as `+towgs84` in the position-vector convention, as the PostGIS entries have it
//! (VN-2000, the Argentine and Austrian datums …). EPSG:9311 (NAD27 / US National Atlas Equal
//! Area) has none: its `+datum=NAD27` stands for grid files proj4rs can't load, and a Helmert
//! with its `+R_A` sphere goes kilometres wrong in proj4rs, so NAD27 is taken as WGS 84 there,
//! tens of metres off, as it is for its predecessor EPSG:2163 in crs-definitions. The Saba and
//! Sint Eustatius Helmerts PROJ marks `+exact` (rotations of hundreds of arc-seconds); in the
//! small-angle form `+towgs84` carries they come out 2–8 m off.

/// The PROJ definition of an EPSG code in the gap, or `None`.
pub fn proj4(code: u16) -> Option<&'static str> {
    DEFS.binary_search_by_key(&code, |&(c, _)| c).ok().and_then(|i| DEFS.get(i)).map(|&(_, p)| p)
}

/// (code, PROJ definition), sorted by code. The comment is the EPSG name.
#[rustfmt::skip]
pub(crate) const DEFS: &[(u16, &str)] = &[
    (9039, "+proj=laea +lat_0=52 +lon_0=10 +x_0=4321000 +y_0=3210000 +ellps=GRS80 +units=m +no_defs"), // ISN2016 / LAEA Europe
    (9040, "+proj=lcc +lat_0=52 +lon_0=10 +lat_1=35 +lat_2=65 +x_0=4000000 +y_0=2800000 +ellps=GRS80 +units=m +no_defs"), // ISN2016 / LCC Europe
    (9053, "+proj=longlat +ellps=WGS84 +no_defs"), // WGS 84 (G730)
    (9054, "+proj=longlat +ellps=WGS84 +no_defs"), // WGS 84 (G873)
    (9055, "+proj=longlat +ellps=WGS84 +no_defs"), // WGS 84 (G1150)
    (9056, "+proj=longlat +ellps=WGS84 +no_defs"), // WGS 84 (G1674)
    (9057, "+proj=longlat +ellps=WGS84 +no_defs"), // WGS 84 (G1762)
    (9059, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRF89
    (9060, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRF90
    (9061, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRF91
    (9062, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRF92
    (9063, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRF93
    (9064, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRF94
    (9065, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRF96
    (9066, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRF97
    (9067, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRF2000
    (9068, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRF2005
    (9069, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRF2014
    (9072, "+proj=longlat +ellps=GRS80 +no_defs"), // NAD83(MARP00)
    (9075, "+proj=longlat +ellps=GRS80 +no_defs"), // NAD83(PACP00)
    (9140, "+proj=longlat +ellps=GRS80 +no_defs"), // KOSOVAREF01
    (9141, "+proj=tmerc +lat_0=0 +lon_0=21 +k=0.9999 +x_0=7500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // KOSOVAREF01 / Balkans zone 7
    (9148, "+proj=longlat +ellps=GRS80 +no_defs"), // SIRGAS-Chile 2013
    (9149, "+proj=utm +zone=18 +south +ellps=GRS80 +units=m +no_defs"), // SIRGAS-Chile 2013 / UTM zone 18S
    (9150, "+proj=utm +zone=19 +south +ellps=GRS80 +units=m +no_defs"), // SIRGAS-Chile 2013 / UTM zone 19S
    (9153, "+proj=longlat +ellps=GRS80 +no_defs"), // SIRGAS-Chile 2016
    (9154, "+proj=utm +zone=18 +south +ellps=GRS80 +units=m +no_defs"), // SIRGAS-Chile 2016 / UTM zone 18S
    (9155, "+proj=utm +zone=19 +south +ellps=GRS80 +units=m +no_defs"), // SIRGAS-Chile 2016 / UTM zone 19S
    (9156, "+proj=utm +zone=32 +south +ellps=GRS80 +units=m +no_defs"), // RSAO13 / UTM zone 32S
    (9157, "+proj=utm +zone=33 +south +ellps=GRS80 +units=m +no_defs"), // RSAO13 / UTM zone 33S
    (9158, "+proj=utm +zone=34 +south +ellps=GRS80 +units=m +no_defs"), // RSAO13 / UTM zone 34S
    (9159, "+proj=tmerc +lat_0=0 +lon_0=12 +k=0.9996 +x_0=500000 +y_0=10000000 +ellps=GRS80 +units=m +no_defs"), // RSAO13 / TM 12 SE
    (9191, "+proj=aea +lat_0=-40 +lon_0=175 +lat_1=-30 +lat_2=-50 +x_0=0 +y_0=0 +datum=WGS84 +units=m +no_defs"), // WGS 84 / NIWA Albers
    (9205, "+proj=tmerc +lat_0=0 +lon_0=103 +k=0.9999 +x_0=500000 +y_0=0 +ellps=WGS84 +towgs84=-191.90441429,-39.30318279,-111.45032835,0.00928836,-0.01975479,0.00427372,0.252906278 +units=m +no_defs"), // VN-2000 / TM-3 103-00
    (9206, "+proj=tmerc +lat_0=0 +lon_0=104 +k=0.9999 +x_0=500000 +y_0=0 +ellps=WGS84 +towgs84=-191.90441429,-39.30318279,-111.45032835,0.00928836,-0.01975479,0.00427372,0.252906278 +units=m +no_defs"), // VN-2000 / TM-3 104-00
    (9207, "+proj=tmerc +lat_0=0 +lon_0=104.5 +k=0.9999 +x_0=500000 +y_0=0 +ellps=WGS84 +towgs84=-191.90441429,-39.30318279,-111.45032835,0.00928836,-0.01975479,0.00427372,0.252906278 +units=m +no_defs"), // VN-2000 / TM-3 104-30
    (9208, "+proj=tmerc +lat_0=0 +lon_0=104.75 +k=0.9999 +x_0=500000 +y_0=0 +ellps=WGS84 +towgs84=-191.90441429,-39.30318279,-111.45032835,0.00928836,-0.01975479,0.00427372,0.252906278 +units=m +no_defs"), // VN-2000 / TM-3 104-45
    (9209, "+proj=tmerc +lat_0=0 +lon_0=105.5 +k=0.9999 +x_0=500000 +y_0=0 +ellps=WGS84 +towgs84=-191.90441429,-39.30318279,-111.45032835,0.00928836,-0.01975479,0.00427372,0.252906278 +units=m +no_defs"), // VN-2000 / TM-3 105-30
    (9210, "+proj=tmerc +lat_0=0 +lon_0=105.75 +k=0.9999 +x_0=500000 +y_0=0 +ellps=WGS84 +towgs84=-191.90441429,-39.30318279,-111.45032835,0.00928836,-0.01975479,0.00427372,0.252906278 +units=m +no_defs"), // VN-2000 / TM-3 105-45
    (9211, "+proj=tmerc +lat_0=0 +lon_0=106 +k=0.9999 +x_0=500000 +y_0=0 +ellps=WGS84 +towgs84=-191.90441429,-39.30318279,-111.45032835,0.00928836,-0.01975479,0.00427372,0.252906278 +units=m +no_defs"), // VN-2000 / TM-3 106-00
    (9212, "+proj=tmerc +lat_0=0 +lon_0=106.25 +k=0.9999 +x_0=500000 +y_0=0 +ellps=WGS84 +towgs84=-191.90441429,-39.30318279,-111.45032835,0.00928836,-0.01975479,0.00427372,0.252906278 +units=m +no_defs"), // VN-2000 / TM-3 106-15
    (9213, "+proj=tmerc +lat_0=0 +lon_0=106.5 +k=0.9999 +x_0=500000 +y_0=0 +ellps=WGS84 +towgs84=-191.90441429,-39.30318279,-111.45032835,0.00928836,-0.01975479,0.00427372,0.252906278 +units=m +no_defs"), // VN-2000 / TM-3 106-30
    (9214, "+proj=tmerc +lat_0=0 +lon_0=107 +k=0.9999 +x_0=500000 +y_0=0 +ellps=WGS84 +towgs84=-191.90441429,-39.30318279,-111.45032835,0.00928836,-0.01975479,0.00427372,0.252906278 +units=m +no_defs"), // VN-2000 / TM-3 107-00
    (9215, "+proj=tmerc +lat_0=0 +lon_0=107.25 +k=0.9999 +x_0=500000 +y_0=0 +ellps=WGS84 +towgs84=-191.90441429,-39.30318279,-111.45032835,0.00928836,-0.01975479,0.00427372,0.252906278 +units=m +no_defs"), // VN-2000 / TM-3 107-15
    (9216, "+proj=tmerc +lat_0=0 +lon_0=107.5 +k=0.9999 +x_0=500000 +y_0=0 +ellps=WGS84 +towgs84=-191.90441429,-39.30318279,-111.45032835,0.00928836,-0.01975479,0.00427372,0.252906278 +units=m +no_defs"), // VN-2000 / TM-3 107-30
    (9217, "+proj=tmerc +lat_0=0 +lon_0=108.25 +k=0.9999 +x_0=500000 +y_0=0 +ellps=WGS84 +towgs84=-191.90441429,-39.30318279,-111.45032835,0.00928836,-0.01975479,0.00427372,0.252906278 +units=m +no_defs"), // VN-2000 / TM-3 108-15
    (9218, "+proj=tmerc +lat_0=0 +lon_0=108.5 +k=0.9999 +x_0=500000 +y_0=0 +ellps=WGS84 +towgs84=-191.90441429,-39.30318279,-111.45032835,0.00928836,-0.01975479,0.00427372,0.252906278 +units=m +no_defs"), // VN-2000 / TM-3 108-30
    (9221, "+proj=aea +lat_0=-30 +lon_0=25 +lat_1=-22 +lat_2=-38 +x_0=1400000 +y_0=1300000 +ellps=WGS84 +units=m +no_defs"), // Hartebeesthoek94 / ZAF BSU Albers 25E
    (9222, "+proj=aea +lat_0=-42 +lon_0=44 +lat_1=-34 +lat_2=-50 +x_0=1200000 +y_0=1300000 +ellps=WGS84 +units=m +no_defs"), // Hartebeesthoek94 / ZAF BSU Albers 44E
    (9248, "+proj=longlat +ellps=intl +towgs84=-192.26,65.72,132.08,0,0,0,0 +no_defs"), // Tapi Aike
    (9249, "+proj=tmerc +lat_0=-90 +lon_0=-72 +k=1 +x_0=1500000 +y_0=0 +ellps=intl +towgs84=-192.26,65.72,132.08,0,0,0,0 +units=m +no_defs"), // Tapi Aike / Argentina 1
    (9250, "+proj=tmerc +lat_0=-90 +lon_0=-69 +k=1 +x_0=2500000 +y_0=0 +ellps=intl +towgs84=-192.26,65.72,132.08,0,0,0,0 +units=m +no_defs"), // Tapi Aike / Argentina 2
    (9251, "+proj=longlat +ellps=intl +towgs84=-9.5,122.9,138.2,0,0,0,0 +no_defs"), // MMN
    (9252, "+proj=tmerc +lat_0=-90 +lon_0=-69 +k=1 +x_0=2500000 +y_0=0 +ellps=intl +towgs84=-9.5,122.9,138.2,0,0,0,0 +units=m +no_defs"), // MMN / Argentina 2
    (9253, "+proj=longlat +ellps=intl +towgs84=-78.1,101.6,133.3,0,0,0,0 +no_defs"), // MMS
    (9254, "+proj=tmerc +lat_0=-90 +lon_0=-69 +k=1 +x_0=2500000 +y_0=0 +ellps=intl +towgs84=-78.1,101.6,133.3,0,0,0,0 +units=m +no_defs"), // MMS / Argentina 2
    (9265, "+proj=utm +zone=19 +south +ellps=WGS84 +towgs84=-0.41,0.46,-0.35,0,0,0,0 +units=m +no_defs"), // POSGAR 2007 / UTM zone 19S
    (9271, "+proj=tmerc +lat_0=0 +lon_0=10.3333333333333 +k=1 +x_0=0 +y_0=0 +ellps=bessel +towgs84=577.326,90.129,463.919,5.137,1.474,5.297,2.4232 +units=m +no_defs"), // MGI / Austria West
    (9272, "+proj=tmerc +lat_0=0 +lon_0=13.3333333333333 +k=1 +x_0=0 +y_0=0 +ellps=bessel +towgs84=577.326,90.129,463.919,5.137,1.474,5.297,2.4232 +units=m +no_defs"), // MGI / Austria Central
    (9273, "+proj=tmerc +lat_0=0 +lon_0=16.3333333333333 +k=1 +x_0=0 +y_0=0 +ellps=bessel +towgs84=577.326,90.129,463.919,5.137,1.474,5.297,2.4232 +units=m +no_defs"), // MGI / Austria East
    (9284, "+proj=tmerc +lat_0=-90 +lon_0=-72 +k=1 +x_0=1500000 +y_0=0 +ellps=intl +towgs84=-233.43,6.65,173.64,0,0,0,0 +units=m +no_defs"), // Pampa del Castillo / Argentina 1
    (9285, "+proj=tmerc +lat_0=-90 +lon_0=-66 +k=1 +x_0=3500000 +y_0=0 +ellps=intl +towgs84=-233.43,6.65,173.64,0,0,0,0 +units=m +no_defs"), // Pampa del Castillo / Argentina 3
    (9294, "+proj=longlat +ellps=GRS80 +towgs84=1.16835,-1.42001,-2.24431,-0.00822,-0.05508,0.01818,0.23388 +no_defs"), // ONGD17
    (9295, "+proj=utm +zone=39 +ellps=GRS80 +towgs84=1.16835,-1.42001,-2.24431,-0.00822,-0.05508,0.01818,0.23388 +units=m +no_defs"), // ONGD17 / UTM zone 39N
    (9296, "+proj=utm +zone=40 +ellps=GRS80 +towgs84=1.16835,-1.42001,-2.24431,-0.00822,-0.05508,0.01818,0.23388 +units=m +no_defs"), // ONGD17 / UTM zone 40N
    (9297, "+proj=utm +zone=41 +ellps=GRS80 +towgs84=1.16835,-1.42001,-2.24431,-0.00822,-0.05508,0.01818,0.23388 +units=m +no_defs"), // ONGD17 / UTM zone 41N
    (9299, "+proj=longlat +ellps=GRS80 +no_defs"), // HS2-IRF
    (9300, "+proj=tmerc +lat_0=52.3 +lon_0=-1.5 +k=1 +x_0=198873.0046 +y_0=375064.3871 +ellps=GRS80 +units=m +no_defs"), // HS2 Survey Grid
    (9309, "+proj=longlat +ellps=GRS80 +no_defs"), // ATRF2014
    (9311, "+proj=laea +R_A +lat_0=45 +lon_0=-100 +x_0=0 +y_0=0 +ellps=clrk66 +units=m +no_defs"), // NAD27 / US National Atlas Equal Area (no datum shift: tens of metres off, see above)
    (9333, "+proj=longlat +ellps=GRS80 +towgs84=0,0,0,-0.008393,0.000749,-0.010276,0 +no_defs"), // KSA-GRF17
    (9354, "+proj=stere +lat_0=-90 +lat_ts=-65 +lon_0=0 +x_0=0 +y_0=0 +datum=WGS84 +units=m +no_defs"), // WGS 84 / IBCSO Polar Stereographic
    (9356, "+proj=utm +zone=36 +ellps=GRS80 +towgs84=0,0,0,-0.008393,0.000749,-0.010276,0 +units=m +no_defs"), // KSA-GRF17 / UTM zone 36N
    (9357, "+proj=utm +zone=37 +ellps=GRS80 +towgs84=0,0,0,-0.008393,0.000749,-0.010276,0 +units=m +no_defs"), // KSA-GRF17 / UTM zone 37N
    (9358, "+proj=utm +zone=38 +ellps=GRS80 +towgs84=0,0,0,-0.008393,0.000749,-0.010276,0 +units=m +no_defs"), // KSA-GRF17 / UTM zone 38N
    (9359, "+proj=utm +zone=39 +ellps=GRS80 +towgs84=0,0,0,-0.008393,0.000749,-0.010276,0 +units=m +no_defs"), // KSA-GRF17 / UTM zone 39N
    (9360, "+proj=utm +zone=40 +ellps=GRS80 +towgs84=0,0,0,-0.008393,0.000749,-0.010276,0 +units=m +no_defs"), // KSA-GRF17 / UTM zone 40N
    (9364, "+proj=longlat +ellps=GRS80 +no_defs"), // TPEN11-IRF
    (9367, "+proj=tmerc +lat_0=53.5 +lon_0=-2.25 +k=1 +x_0=203252.175 +y_0=407512.765 +ellps=GRS80 +units=m +no_defs"), // TPEN11 Grid
    (9372, "+proj=longlat +ellps=GRS80 +no_defs"), // MML07-IRF
    (9373, "+proj=tmerc +lat_0=52.45 +lon_0=-0.85 +k=1 +x_0=49350.157 +y_0=108398.212 +ellps=GRS80 +units=m +no_defs"), // MML07 Grid
    (9377, "+proj=tmerc +lat_0=4 +lon_0=-73 +k=0.9992 +x_0=5000000 +y_0=2000000 +ellps=GRS80 +units=m +no_defs"), // MAGNA-SIRGAS 2018 / Origen-Nacional
    (9380, "+proj=longlat +ellps=GRS80 +no_defs"), // IGb14
    (9384, "+proj=longlat +ellps=GRS80 +no_defs"), // AbInvA96_2020-IRF
    (9387, "+proj=tmerc +lat_0=57.4 +lon_0=-3.2 +k=1 +x_0=155828.702 +y_0=115225.707 +ellps=GRS80 +units=m +no_defs"), // AbInvA96_2020 Grid
    (9391, "+proj=utm +zone=35 +ellps=GRS80 +units=m +no_defs"), // BGS2005 / UTM zone 35N
    (9403, "+proj=longlat +ellps=intl +towgs84=-307,-92,127,0,0,0,0 +no_defs"), // PN68
    (9404, "+proj=utm +zone=27 +ellps=intl +towgs84=-307,-92,127,0,0,0,0 +units=m +no_defs"), // PN68 / UTM zone 27N
    (9405, "+proj=utm +zone=28 +ellps=intl +towgs84=-307,-92,127,0,0,0,0 +units=m +no_defs"), // PN68 / UTM zone 28N
    (9406, "+proj=utm +zone=27 +ellps=intl +towgs84=-307,-92,127,0,0,0,0 +units=m +no_defs"), // PN84 / UTM zone 27N
    (9407, "+proj=utm +zone=28 +ellps=intl +towgs84=-307,-92,127,0,0,0,0 +units=m +no_defs"), // PN84 / UTM zone 28N
    (9453, "+proj=longlat +ellps=GRS80 +no_defs"), // GBK19-IRF
    (9456, "+proj=tmerc +lat_0=55.75 +lon_0=-4.35 +k=1 +x_0=93720.394 +y_0=113870.493 +ellps=GRS80 +units=m +no_defs"), // GBK19 Grid
    (9470, "+proj=longlat +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +no_defs"), // SRGI2013
    (9473, "+proj=aea +lat_0=0 +lon_0=132 +lat_1=-18 +lat_2=-36 +x_0=0 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // GDA2020 / Australian Albers
    (9474, "+proj=longlat +a=6378136 +rf=298.257839303 +no_defs"), // PZ-90.02
    (9475, "+proj=longlat +a=6378136 +rf=298.257839303 +no_defs"), // PZ-90.11
    (9476, "+proj=utm +zone=46 +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +units=m +no_defs"), // SRGI2013 / UTM zone 46N
    (9477, "+proj=utm +zone=47 +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +units=m +no_defs"), // SRGI2013 / UTM zone 47N
    (9478, "+proj=utm +zone=48 +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +units=m +no_defs"), // SRGI2013 / UTM zone 48N
    (9479, "+proj=utm +zone=49 +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +units=m +no_defs"), // SRGI2013 / UTM zone 49N
    (9480, "+proj=utm +zone=50 +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +units=m +no_defs"), // SRGI2013 / UTM zone 50N
    (9481, "+proj=utm +zone=51 +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +units=m +no_defs"), // SRGI2013 / UTM zone 51N
    (9482, "+proj=utm +zone=52 +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +units=m +no_defs"), // SRGI2013 / UTM zone 52N
    (9487, "+proj=utm +zone=47 +south +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +units=m +no_defs"), // SRGI2013 / UTM zone 47S
    (9488, "+proj=utm +zone=48 +south +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +units=m +no_defs"), // SRGI2013 / UTM zone 48S
    (9489, "+proj=utm +zone=49 +south +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +units=m +no_defs"), // SRGI2013 / UTM zone 49S
    (9490, "+proj=utm +zone=50 +south +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +units=m +no_defs"), // SRGI2013 / UTM zone 50S
    (9491, "+proj=utm +zone=51 +south +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +units=m +no_defs"), // SRGI2013 / UTM zone 51S
    (9492, "+proj=utm +zone=52 +south +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +units=m +no_defs"), // SRGI2013 / UTM zone 52S
    (9493, "+proj=utm +zone=53 +south +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +units=m +no_defs"), // SRGI2013 / UTM zone 53S
    (9494, "+proj=utm +zone=54 +south +ellps=WGS84 +towgs84=-0.2773,0.0534,0.4819,-0.0192857593841035,0.00589917345866696,-0.00199870597253436,-0.028 +units=m +no_defs"), // SRGI2013 / UTM zone 54S
    (9498, "+proj=tmerc +lat_0=-34.6292666666667 +lon_0=-58.4633083333333 +k=1 +x_0=20000 +y_0=70000 +ellps=WGS84 +towgs84=-0.41,0.46,-0.35,0,0,0,0 +units=m +no_defs"), // POSGAR 2007 / CABA 2019
    (9547, "+proj=longlat +ellps=GRS80 +no_defs"), // LTF2004(G)
    (9549, "+proj=lcc +lat_1=44.3791666666667 +k_0=1 +lat_0=45.1833333333333 +lon_0=6.81666666666667 +x_0=150000 +y_0=50000 +ellps=GRS80 +units=m +no_defs"), // LTF2004(C)
    (9674, "+proj=aea +lat_0=34 +lon_0=-120 +lat_1=43 +lat_2=48 +x_0=600000 +y_0=0 +datum=NAD83 +units=m +no_defs"), // NAD83 / USFS R6 Albers
    (9678, "+proj=tmerc +lat_0=0 +lon_0=90 +k=0.9996 +x_0=500000 +y_0=-2000000 +ellps=evrst30 +towgs84=283.729,735.942,261.143,0,0,0,0 +units=m +no_defs"), // Gulshan 303 / Bangladesh Transverse Mercator
    (9680, "+proj=tmerc +lat_0=0 +lon_0=90 +k=0.9996 +x_0=500000 +y_0=0 +datum=WGS84 +units=m +no_defs"), // WGS 84 / TM 90 NE
    (9696, "+proj=longlat +ellps=GRS80 +no_defs"), // REDGEOMIN
    (9697, "+proj=utm +zone=12 +south +ellps=GRS80 +units=m +no_defs"), // REDGEOMIN / UTM zone 12S
    (9698, "+proj=utm +zone=18 +south +ellps=GRS80 +units=m +no_defs"), // REDGEOMIN / UTM zone 18S
    (9699, "+proj=utm +zone=19 +south +ellps=GRS80 +units=m +no_defs"), // REDGEOMIN / UTM zone 19S
    (9702, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRF2000-PL
    (9709, "+proj=utm +zone=23 +ellps=GRS80 +towgs84=-0.991,1.9072,0.5129,0.0257899075194932,0.0096500989602704,0.0116599432323421,0 +units=m +no_defs"), // NAD83(CSRS) / UTM zone 23N
    (9712, "+proj=utm +zone=24 +datum=NAD83 +units=m +no_defs"), // NAD83 / UTM zone 24N
    (9713, "+proj=utm +zone=24 +ellps=GRS80 +towgs84=-0.991,1.9072,0.5129,0.0257899075194932,0.0096500989602704,0.0116599432323421,0 +units=m +no_defs"), // NAD83(CSRS) / UTM zone 24N
    (9716, "+proj=utm +zone=34 +ellps=GRS80 +units=m +no_defs"), // IGM95 / UTM zone 34N
    (9739, "+proj=longlat +ellps=GRS80 +no_defs"), // EOS21-IRF
    (9741, "+proj=tmerc +lat_0=56.35 +lon_0=-2.75 +k=1 +x_0=74996.927 +y_0=133508.35 +ellps=GRS80 +units=m +no_defs"), // EOS21 Grid
    (9748, "+proj=tmerc +lat_0=30.5 +lon_0=-85.8333333333333 +k=0.99996 +x_0=200000.0001016 +y_0=0 +ellps=GRS80 +units=us-ft +no_defs"), // NAD83(2011) / Alabama East (ftUS)
    (9749, "+proj=tmerc +lat_0=30 +lon_0=-87.5 +k=0.999933333 +x_0=600000 +y_0=0 +ellps=GRS80 +units=us-ft +no_defs"), // NAD83(2011) / Alabama West (ftUS)
    (9755, "+proj=longlat +ellps=WGS84 +no_defs"), // WGS 84 (G2139)
    (9758, "+proj=longlat +ellps=GRS80 +no_defs"), // ECML14_NB-IRF
    (9761, "+proj=tmerc +lat_0=55.05 +lon_0=-1.55 +k=1 +x_0=112242.8512 +y_0=402313.7432 +ellps=GRS80 +units=m +no_defs"), // ECML14_NB Grid
    (9763, "+proj=longlat +ellps=GRS80 +no_defs"), // EWR2-IRF
    (9766, "+proj=tmerc +lat_0=51.95 +lon_0=-0.9 +k=1 +x_0=192519.9715 +y_0=146942.6806 +ellps=GRS80 +units=m +no_defs"), // EWR2 Grid
    (9777, "+proj=longlat +ellps=GRS80 +no_defs"), // RGF93 v2
    (9779, "+proj=longlat +ellps=GRS80 +no_defs"), // RGF93 v2 (lon-lat)
    (9782, "+proj=longlat +ellps=GRS80 +no_defs"), // RGF93 v2b
    (9784, "+proj=longlat +ellps=GRS80 +no_defs"), // RGF93 v2b (lon-lat)
    (9793, "+proj=lcc +lat_0=46.5 +lon_0=3 +lat_1=49 +lat_2=44 +x_0=700000 +y_0=6600000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2 / Lambert-93
    (9794, "+proj=lcc +lat_0=46.5 +lon_0=3 +lat_1=49 +lat_2=44 +x_0=700000 +y_0=6600000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2b / Lambert-93
    (9821, "+proj=tmerc +lat_0=0 +lon_0=30.5 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-32 Kyiv region
    (9822, "+proj=lcc +lat_0=42 +lon_0=3 +lat_1=41.25 +lat_2=42.75 +x_0=1700000 +y_0=1200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2 / CC42
    (9823, "+proj=lcc +lat_0=43 +lon_0=3 +lat_1=42.25 +lat_2=43.75 +x_0=1700000 +y_0=2200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2 / CC43
    (9824, "+proj=lcc +lat_0=44 +lon_0=3 +lat_1=43.25 +lat_2=44.75 +x_0=1700000 +y_0=3200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2 / CC44
    (9825, "+proj=lcc +lat_0=45 +lon_0=3 +lat_1=44.25 +lat_2=45.75 +x_0=1700000 +y_0=4200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2 / CC45
    (9826, "+proj=lcc +lat_0=46 +lon_0=3 +lat_1=45.25 +lat_2=46.75 +x_0=1700000 +y_0=5200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2 / CC46
    (9827, "+proj=lcc +lat_0=47 +lon_0=3 +lat_1=46.25 +lat_2=47.75 +x_0=1700000 +y_0=6200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2 / CC47
    (9828, "+proj=lcc +lat_0=48 +lon_0=3 +lat_1=47.25 +lat_2=48.75 +x_0=1700000 +y_0=7200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2 / CC48
    (9829, "+proj=lcc +lat_0=49 +lon_0=3 +lat_1=48.25 +lat_2=49.75 +x_0=1700000 +y_0=8200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2 / CC49
    (9830, "+proj=lcc +lat_0=50 +lon_0=3 +lat_1=49.25 +lat_2=50.75 +x_0=1700000 +y_0=9200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2 / CC50
    (9831, "+proj=tmerc +lat_0=0 +lon_0=34.5 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-01 Crimea
    (9832, "+proj=tmerc +lat_0=0 +lon_0=28.6666666666667 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-05 Vinnytsia
    (9833, "+proj=tmerc +lat_0=0 +lon_0=24.8333333333333 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-07 Volyn
    (9834, "+proj=tmerc +lat_0=0 +lon_0=35 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-12 Dnipropetrovsk
    (9835, "+proj=tmerc +lat_0=0 +lon_0=37.5 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-14 Donetsk
    (9836, "+proj=tmerc +lat_0=0 +lon_0=28.5 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-18 Zhytomyr
    (9837, "+proj=tmerc +lat_0=0 +lon_0=23.5 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-21 Zakarpattia
    (9838, "+proj=tmerc +lat_0=0 +lon_0=36 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-23 Zaporizhzhia
    (9839, "+proj=tmerc +lat_0=0 +lon_0=24.75 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-26 Ivano-Frankivsk
    (9840, "+proj=tmerc +lat_0=0 +lon_0=32 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-35 Kirovohrad
    (9841, "+proj=tmerc +lat_0=0 +lon_0=39 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-44 Luhansk
    (9842, "+proj=lcc +lat_0=42 +lon_0=3 +lat_1=41.25 +lat_2=42.75 +x_0=1700000 +y_0=1200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2b / CC42
    (9843, "+proj=lcc +lat_0=43 +lon_0=3 +lat_1=42.25 +lat_2=43.75 +x_0=1700000 +y_0=2200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2b / CC43
    (9844, "+proj=lcc +lat_0=44 +lon_0=3 +lat_1=43.25 +lat_2=44.75 +x_0=1700000 +y_0=3200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2b / CC44
    (9845, "+proj=lcc +lat_0=45 +lon_0=3 +lat_1=44.25 +lat_2=45.75 +x_0=1700000 +y_0=4200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2b / CC45
    (9846, "+proj=lcc +lat_0=46 +lon_0=3 +lat_1=45.25 +lat_2=46.75 +x_0=1700000 +y_0=5200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2b / CC46
    (9847, "+proj=lcc +lat_0=47 +lon_0=3 +lat_1=46.25 +lat_2=47.75 +x_0=1700000 +y_0=6200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2b / CC47
    (9848, "+proj=lcc +lat_0=48 +lon_0=3 +lat_1=47.25 +lat_2=48.75 +x_0=1700000 +y_0=7200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2b / CC48
    (9849, "+proj=lcc +lat_0=49 +lon_0=3 +lat_1=48.25 +lat_2=49.75 +x_0=1700000 +y_0=8200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2b / CC49
    (9850, "+proj=lcc +lat_0=50 +lon_0=3 +lat_1=49.25 +lat_2=50.75 +x_0=1700000 +y_0=9200000 +ellps=GRS80 +units=m +no_defs"), // RGF93 v2b / CC50
    (9851, "+proj=tmerc +lat_0=0 +lon_0=24 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-46 Lviv
    (9852, "+proj=tmerc +lat_0=0 +lon_0=31.8333333333333 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-48 Mykolaiv
    (9853, "+proj=tmerc +lat_0=0 +lon_0=30 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-51 Odessa
    (9854, "+proj=tmerc +lat_0=0 +lon_0=33.8333333333333 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-53 Poltava
    (9855, "+proj=tmerc +lat_0=0 +lon_0=27 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-56 Rivne
    (9856, "+proj=tmerc +lat_0=0 +lon_0=34.5 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-59 Sumy
    (9857, "+proj=tmerc +lat_0=0 +lon_0=25.5 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-61 Ternopil
    (9858, "+proj=tmerc +lat_0=0 +lon_0=36.5 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-63 Kharkiv
    (9859, "+proj=tmerc +lat_0=0 +lon_0=33.5 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-65 Kherson
    (9860, "+proj=tmerc +lat_0=0 +lon_0=27 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-68 Khmelnytsky
    (9861, "+proj=tmerc +lat_0=0 +lon_0=31.5 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-71 Cherkasy
    (9862, "+proj=tmerc +lat_0=0 +lon_0=26 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-73 Chernivtsi
    (9863, "+proj=tmerc +lat_0=0 +lon_0=32 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-74 Chernihiv
    (9864, "+proj=tmerc +lat_0=0 +lon_0=30.5 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-80 Kyiv city
    (9865, "+proj=tmerc +lat_0=0 +lon_0=33 +k=1 +x_0=300000 +y_0=0 +ellps=krass +towgs84=24,-121,-76,0,0,0,0 +units=m +no_defs"), // UCS-2000 / LCS-85 Sevastopol
    (9866, "+proj=longlat +ellps=GRS80 +no_defs"), // MRH21-IRF
    (9869, "+proj=tmerc +lat_0=52.3 +lon_0=-1.8 +k=1 +x_0=227286.9881 +y_0=265751.2874 +ellps=GRS80 +units=m +no_defs"), // MRH21 Grid
    (9871, "+proj=longlat +ellps=GRS80 +no_defs"), // MOLDOR11-IRF
    (9874, "+proj=utm +zone=57 +south +ellps=GRS80 +units=m +no_defs"), // PNG94 / PNGMG94 zone 57
    (9875, "+proj=utm +zone=58 +south +ellps=GRS80 +units=m +no_defs"), // PNG94 / PNGMG94 zone 58
    (9880, "+proj=tmerc +lat_0=53.35 +lon_0=-1.9 +k=1 +x_0=226574.2032 +y_0=390894.838 +ellps=GRS80 +units=m +no_defs"), // MOLDOR11 Grid
    (9895, "+proj=tmerc +lat_0=49.8333333333333 +lon_0=6.16666666666667 +k=1 +x_0=80000 +y_0=100000 +ellps=intl +towgs84=-189.6806,18.3463,-42.7695,-0.33746,-3.09264,2.53861,0.4598 +units=m +no_defs"), // LUREF / Luxembourg TM (3D)
    (9939, "+proj=longlat +ellps=GRS80 +no_defs"), // EBBWV14-IRF
    (9943, "+proj=tmerc +lat_0=51.75 +lon_0=-3.1 +k=1 +x_0=106702.326 +y_0=119968.1395 +ellps=GRS80 +units=m +no_defs"), // EBBWV14 Grid
    (9945, "+proj=tmerc +lat_0=0 +lon_0=21 +k=0.9999 +x_0=500000 +y_0=-4000000 +ellps=bessel +towgs84=521.748,229.489,590.921,4.029,4.488,-15.521,-9.78 +units=m +no_defs"), // Macedonia State Coordinate System truncated
    (9947, "+proj=laea +lat_0=65 +lon_0=-19 +x_0=1700000 +y_0=1300000 +ellps=GRS80 +units=m +no_defs"), // ISN2004 / LAEA Iceland
    (9964, "+proj=longlat +ellps=GRS80 +no_defs"), // HULLEE13-IRF
    (9967, "+proj=tmerc +lat_0=53.75 +lon_0=-0.95 +k=1 +x_0=140859.7394 +y_0=247512.2812 +ellps=GRS80 +units=m +no_defs"), // HULLEE13 Grid
    (9969, "+proj=longlat +ellps=GRS80 +no_defs"), // SCM22-IRF
    (9972, "+proj=tmerc +lat_0=56.6 +lon_0=-3.85 +k=1 +x_0=108600.972 +y_0=239087.349 +ellps=GRS80 +units=m +no_defs"), // SCM22 Grid
    (9974, "+proj=longlat +ellps=GRS80 +no_defs"), // FNL22-IRF
    (9977, "+proj=tmerc +lat_0=58 +lon_0=-3.8 +k=1 +x_0=139618.9493 +y_0=183110.794 +ellps=GRS80 +units=m +no_defs"), // FNL22 Grid
    (9990, "+proj=longlat +ellps=GRS80 +no_defs"), // ITRF2020
    (10158, "+proj=longlat +ellps=intl +no_defs"), // S34J-IRF
    (10160, "+proj=tmerc +lat_0=0 +lon_0=10.37 +k=1 +x_0=-210327 +y_0=-6034310 +ellps=intl +units=m +no_defs"), // S34J reconstruction east-orientated
    (10175, "+proj=longlat +ellps=GRS80 +no_defs"), // DoPw22-IRF
    (10178, "+proj=longlat +ellps=GRS80 +no_defs"), // IGS20
    (10183, "+proj=tmerc +lat_0=52.7 +lon_0=-4.15 +k=1 +x_0=64859.6557 +y_0=122266.5277 +ellps=GRS80 +units=m +no_defs"), // DoPw22 Grid
    (10185, "+proj=longlat +ellps=GRS80 +no_defs"), // ShAb07-IRF
    (10188, "+proj=tmerc +lat_0=52.6 +lon_0=-3.35 +k=1 +x_0=56023.5377 +y_0=24567.6764 +ellps=GRS80 +units=m +no_defs"), // ShAb07 Grid
    (10191, "+proj=longlat +ellps=GRS80 +no_defs"), // CNH22-IRF
    (10194, "+proj=lcc +lat_0=53.25 +lon_0=-3.5 +lat_1=53.1 +lat_2=53.4 +x_0=212548.8756 +y_0=495230.9254 +ellps=GRS80 +units=m +no_defs"), // CNH22 Grid
    (10196, "+proj=longlat +ellps=GRS80 +no_defs"), // CWS13-IRF
    (10199, "+proj=tmerc +lat_0=52.4 +lon_0=-2.9 +k=1 +x_0=199668.0926 +y_0=89354.3229 +ellps=GRS80 +units=m +no_defs"), // CWS13 Grid
    (10204, "+proj=longlat +ellps=GRS80 +no_defs"), // DIBA15-IRF
    (10207, "+proj=tmerc +lat_0=51.85 +lon_0=-1.3 +k=1 +x_0=511622.854 +y_0=324737.4633 +ellps=GRS80 +units=m +no_defs"), // DIBA15 Grid
    (10209, "+proj=longlat +ellps=GRS80 +no_defs"), // GWPBS22-IRF
    (10212, "+proj=lcc +lat_0=51.65 +lon_0=-2.65 +lat_1=51.4 +lat_2=51.9 +x_0=168854.016 +y_0=193447.117 +ellps=GRS80 +units=m +no_defs"), // GWPBS22 Grid
    (10214, "+proj=longlat +ellps=GRS80 +no_defs"), // GWWAB22-IRF
    (10217, "+proj=lcc +lat_0=51.65 +lon_0=-2.65 +lat_1=51.4 +lat_2=51.9 +x_0=168854.016 +y_0=193447.117 +ellps=GRS80 +units=m +no_defs"), // GWWAB22 Grid
    (10219, "+proj=longlat +ellps=GRS80 +no_defs"), // GWWWA22-IRF
    (10222, "+proj=lcc +lat_0=51.65 +lon_0=-2.65 +lat_1=51.4 +lat_2=51.9 +x_0=168854.016 +y_0=193447.117 +ellps=GRS80 +units=m +no_defs"), // GWWWA22 Grid
    (10224, "+proj=longlat +ellps=GRS80 +no_defs"), // MALS09-IRF
    (10227, "+proj=tmerc +lat_0=52.2 +lon_0=-1.15 +k=1 +x_0=175262.1809 +y_0=174688.2508 +ellps=GRS80 +units=m +no_defs"), // MALS09 Grid
    (10229, "+proj=longlat +ellps=GRS80 +no_defs"), // OxWo08-IRF
    (10235, "+proj=tmerc +lat_0=51.95 +lon_0=-1.7 +k=1 +x_0=134791.6965 +y_0=121872.5056 +ellps=GRS80 +units=m +no_defs"), // OxWo08 Grid
    (10237, "+proj=longlat +ellps=GRS80 +no_defs"), // SYC20-IRF
    (10240, "+proj=tmerc +lat_0=52.9 +lon_0=-2.6 +k=1 +x_0=110094.4312 +y_0=120623.8396 +ellps=GRS80 +units=m +no_defs"), // SYC20 Grid
    (10249, "+proj=longlat +ellps=intl +no_defs"), // S34S-IRF
    (10250, "+proj=tmerc +lat_0=0 +lon_0=10.37 +k=1 +x_0=-210327 +y_0=-6034310 +ellps=intl +units=m +no_defs"), // S34S reconstruction east-orientated
    (10252, "+proj=longlat +ellps=intl +no_defs"), // S45B-IRF
    (10254, "+proj=tmerc +lat_0=55.11171 +lon_0=14.88927 +k=1 +x_0=-50000 +y_0=50000 +ellps=intl +units=m +no_defs"), // S45B reconstruction east-orientated
    (10256, "+proj=longlat +a=6377019.27 +rf=300 +no_defs"), // GS-IRF
    (10258, "+proj=lcc +lat_1=56 +k_0=1 +lat_0=55 +lon_0=10.37775 +x_0=0 +y_0=0 +a=6377019.27 +rf=300 +units=m +no_defs"), // GS reconstruction east-orientated
    (10260, "+proj=longlat +a=6377019.27 +rf=300 +no_defs"), // GSB-IRF
    (10262, "+proj=lcc +lat_1=56 +k_0=1 +lat_0=55 +lon_0=14.92775 +x_0=-18831.46 +y_0=5614.621 +a=6377019.27 +rf=300 +units=m +no_defs"), // GSB reconstruction east-orientated
    (10265, "+proj=longlat +a=6377019.27 +rf=300 +no_defs"), // KK-IRF
    (10266, "+proj=lcc +lat_1=56 +k_0=1 +lat_0=55 +lon_0=10.37775 +x_0=0 +y_0=0 +a=6377019.27 +rf=300 +units=m +no_defs"), // KK reconstruction east-orientated
    (10268, "+proj=longlat +ellps=bessel +no_defs"), // Ostenfeld-IRF
    (10270, "+proj=tmerc +lat_0=54.4685 +lon_0=9.233 +k=1 +x_0=0 +y_0=0 +ellps=bessel +units=m +no_defs"), // Ostenfeld reconstruction
    (10272, "+proj=longlat +ellps=GRS80 +no_defs"), // SMITB20-IRF
    (10275, "+proj=tmerc +lat_0=50.75 +lon_0=-3.85 +k=1 +x_0=110693.666 +y_0=102089.2943 +ellps=GRS80 +units=m +no_defs"), // SMITB20 Grid
    (10277, "+proj=longlat +ellps=GRS80 +no_defs"), // RBEPP12-IRF
    (10280, "+proj=lcc +lat_0=50.85 +lon_0=-3.25 +lat_1=50.3 +lat_2=51.45 +x_0=372382.8292 +y_0=217764.7796 +ellps=GRS80 +units=m +no_defs"), // RBEPP12 Grid
    (10284, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRS89/DREF91/2016
    (10285, "+proj=tmerc +lat_0=0 +lon_0=9 +k=1 +x_0=3500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // ETRS89/DREF91/2016 / 3-degree Gauss-Kruger zone 3
    (10286, "+proj=tmerc +lat_0=0 +lon_0=3 +k=0.9996 +x_0=31500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // ETRS89/DREF91/2016 / UTM zone 31N (N-zE)
    (10287, "+proj=tmerc +lat_0=0 +lon_0=3 +k=0.9996 +x_0=31500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // ETRS89/DREF91/2016 / UTM zone 31N (zE-N)
    (10288, "+proj=tmerc +lat_0=0 +lon_0=9 +k=0.9996 +x_0=32500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // ETRS89/DREF91/2016 / UTM zone 32N (N-zE)
    (10289, "+proj=tmerc +lat_0=0 +lon_0=9 +k=0.9996 +x_0=32500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // ETRS89/DREF91/2016 / UTM zone 32N (zE-N)
    (10290, "+proj=tmerc +lat_0=0 +lon_0=15 +k=0.9996 +x_0=33500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // ETRS89/DREF91/2016 / UTM zone 33N (N-zE)
    (10291, "+proj=tmerc +lat_0=0 +lon_0=15 +k=0.9996 +x_0=33500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // ETRS89/DREF91/2016 / UTM zone 33N (zE-N)
    (10299, "+proj=longlat +ellps=GRS80 +no_defs"), // RGSH2020
    (10305, "+proj=longlat +ellps=GRS80 +no_defs"), // LKS-2020
    (10306, "+proj=tmerc +lat_0=0 +lon_0=24 +k=0.9996 +x_0=500000 +y_0=-6000000 +ellps=GRS80 +units=m +no_defs"), // LKS-2020 / Latvia TM
    (10307, "+proj=longlat +ellps=GRS80 +no_defs"), // RGNC91-93 (lon-lat)
    (10310, "+proj=longlat +ellps=GRS80 +no_defs"), // RGNC15
    (10312, "+proj=longlat +ellps=GRS80 +no_defs"), // RGNC15 (lon-lat)
    (10314, "+proj=lcc +lat_0=-21.5 +lon_0=166 +lat_1=-20.6666666666667 +lat_2=-22.3333333333333 +x_0=2400000 +y_0=2300000 +ellps=GRS80 +units=m +no_defs"), // RGNC15 / Lambert New Caledonia 2015
    (10315, "+proj=utm +zone=57 +south +ellps=GRS80 +units=m +no_defs"), // RGNC15 / UTM zone 57S
    (10316, "+proj=utm +zone=58 +south +ellps=GRS80 +units=m +no_defs"), // RGNC15 / UTM zone 58S
    (10317, "+proj=utm +zone=59 +south +ellps=GRS80 +units=m +no_defs"), // RGNC15 / UTM zone 59S
    (10328, "+proj=longlat +ellps=GRS80 +no_defs"), // BH_ETRS89
    (10329, "+proj=tmerc +lat_0=0 +lon_0=18 +k=0.9999 +x_0=500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // BH_ETRS89 / TM
    (10345, "+proj=longlat +a=6378273 +b=6356889.449 +no_defs"), // Hughes 1980
    (10346, "+proj=longlat +R=6371228 +no_defs"), // NSIDC Authalic Sphere
    (10414, "+proj=longlat +ellps=GRS80 +no_defs"), // NAD83(CSRS)v8
    (10448, "+proj=tmerc +lat_0=0 +lon_0=117.883333333333 +k=1.0000044 +x_0=50000 +y_0=4000000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / ALB94
    (10449, "+proj=tmerc +lat_0=0 +lon_0=115.25 +k=1.0000022 +x_0=60000 +y_0=2600000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / BIO94
    (10450, "+proj=tmerc +lat_0=0 +lon_0=122.333333333333 +k=1.00000298 +x_0=50000 +y_0=2200000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / BRO94
    (10451, "+proj=tmerc +lat_0=0 +lon_0=115.433333333333 +k=0.99999592 +x_0=50000 +y_0=3900000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / BCG94
    (10452, "+proj=tmerc +lat_0=0 +lon_0=113.666666666667 +k=0.99999796 +x_0=50000 +y_0=2950000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / CARN94
    (10453, "+proj=tmerc +lat_0=0 +lon_0=115.933333333333 +k=1.000019 +x_0=40000 +y_0=4000000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / COL94
    (10454, "+proj=tmerc +lat_0=0 +lon_0=121.883333333333 +k=1.0000055 +x_0=50000 +y_0=3950000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / ESP94
    (10455, "+proj=tmerc +lat_0=0 +lon_0=114.066666666667 +k=1.00000236 +x_0=50000 +y_0=2650000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / EXM94
    (10456, "+proj=tmerc +lat_0=0 +lon_0=114.583333333333 +k=1.00000628 +x_0=50000 +y_0=3350000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / GCG94
    (10457, "+proj=tmerc +lat_0=0 +lon_0=121.5 +k=1.00004949 +x_0=60000 +y_0=3700000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / GOLD94
    (10458, "+proj=tmerc +lat_0=0 +lon_0=114.983333333333 +k=1.00000314 +x_0=50000 +y_0=3550000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / JCG94
    (10459, "+proj=tmerc +lat_0=0 +lon_0=114.315277777778 +k=1.000014 +x_0=55000 +y_0=3600000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / KALB94
    (10460, "+proj=tmerc +lat_0=0 +lon_0=116.933333333333 +k=0.9999989 +x_0=50000 +y_0=2450000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / KAR94
    (10461, "+proj=tmerc +lat_0=0 +lon_0=128.75 +k=1.0000165 +x_0=50000 +y_0=2000000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / KUN94
    (10462, "+proj=tmerc +lat_0=0 +lon_0=115.366666666667 +k=1.00000157 +x_0=50000 +y_0=3650000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / LCG94
    (10463, "+proj=tmerc +lat_0=0 +lon_0=115.166666666667 +k=1.0000055 +x_0=50000 +y_0=3950000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / MRCG94
    (10464, "+proj=tmerc +lat_0=0 +lon_0=115.816666666667 +k=0.99999906 +x_0=50000 +y_0=3800000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / PCG94
    (10465, "+proj=tmerc +lat_0=0 +lon_0=118.6 +k=1.00000135 +x_0=50000 +y_0=2400000 +ellps=GRS80 +units=m +no_defs"), // GDA94 / PHG94
    (10468, "+proj=longlat +ellps=GRS80 +no_defs"), // COV23-IRF
    (10471, "+proj=tmerc +lat_0=52.4 +lon_0=-1.55 +k=1 +x_0=116887.9989 +y_0=102194.9369 +ellps=GRS80 +units=m +no_defs"), // COV23 Grid
    (10475, "+proj=longlat +ellps=WGS84 +no_defs"), // BBT2000
    (10477, "+proj=tmerc +lat_0=46.9807763055556 +lon_0=11.52849375 +k=1.000121 +x_0=20000 +y_0=100000 +ellps=WGS84 +units=m +no_defs"), // BBT2000 / BBT-TM
    (10481, "+proj=aea +lat_0=31.25 +lon_0=-100 +lat_1=27.5 +lat_2=35 +x_0=1500000 +y_0=6000000 +datum=NAD83 +units=us-ft +no_defs"), // NAD83 / TWDB GM
    (10516, "+proj=tmerc +lat_0=44.2533353222222 +lon_0=-90.8442965138889 +k=1.0000353 +x_0=27000 +y_0=24999.9998983998 +ellps=GRS80 +units=us-ft +no_defs"), // NAD83(2011) / Adjusted Jackson (ftUS)
    (10571, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRF2020
    (10592, "+proj=laea +lat_0=5 +lon_0=20 +x_0=0 +y_0=0 +datum=WGS84 +units=m +no_defs"), // WGS 84 / GLANCE Africa
    (10594, "+proj=laea +lat_0=45 +lon_0=100 +x_0=0 +y_0=0 +datum=WGS84 +units=m +no_defs"), // WGS 84 / GLANCE Asia
    (10596, "+proj=laea +lat_0=55 +lon_0=20 +x_0=0 +y_0=0 +datum=WGS84 +units=m +no_defs"), // WGS 84 / GLANCE Europe
    (10598, "+proj=laea +lat_0=50 +lon_0=-100 +x_0=0 +y_0=0 +datum=WGS84 +units=m +no_defs"), // WGS 84 / GLANCE North America
    (10601, "+proj=laea +lat_0=-15 +lon_0=135 +x_0=0 +y_0=0 +datum=WGS84 +units=m +no_defs"), // WGS 84 / GLANCE Oceania
    (10603, "+proj=laea +lat_0=-15 +lon_0=-60 +x_0=0 +y_0=0 +datum=WGS84 +units=m +no_defs"), // WGS 84 / GLANCE South America
    (10606, "+proj=longlat +ellps=WGS84 +no_defs"), // WGS 84 (G2296)
    (10623, "+proj=longlat +ellps=GRS80 +no_defs"), // ECML14-IRF
    (10626, "+proj=tmerc +lat_0=53.8 +lon_0=-1.6 +k=1 +x_0=108021.121 +y_0=263196.8721 +ellps=GRS80 +units=m +no_defs"), // ECML14 Grid
    (10628, "+proj=longlat +ellps=GRS80 +no_defs"), // WC05-IRF
    (10632, "+proj=tmerc +lat_0=53.45 +lon_0=-2.15 +k=1 +x_0=209900.2337 +y_0=401958.2494 +ellps=GRS80 +units=m +no_defs"), // WC05 Grid
    (10636, "+proj=longlat +ellps=intl +towgs84=1138.7432,-2064.4761,110.7016,214.615206,-479.360036,164.703951,-402.32073 +no_defs"), // Saba (Helmert is +exact in PROJ: 2–8 m off here)
    (10639, "+proj=longlat +ellps=GRS80 +no_defs"), // BES2020 Saba
    (10641, "+proj=tmerc +lat_0=0 +lon_0=-63 +k=0.9996 +x_0=29973.97 +y_0=-1947925.94 +ellps=intl +towgs84=1138.7432,-2064.4761,110.7016,214.615206,-479.360036,164.703951,-402.32073 +units=m +no_defs"), // Saba DPnet (Helmert is +exact in PROJ: 2–8 m off here)
    (10665, "+proj=tmerc +lat_0=0 +lon_0=-51 +k=0.999995 +x_0=300000 +y_0=5000000 +ellps=GRS80 +units=m +no_defs"), // SIRGAS 2000 / Porto Alegre TM
    (10671, "+proj=longlat +ellps=GRS80 +no_defs"), // RGM23
    (10673, "+proj=longlat +ellps=GRS80 +no_defs"), // RGM23 (lon-lat)
    (10674, "+proj=utm +zone=38 +south +ellps=GRS80 +units=m +no_defs"), // RGM23 / UTM zone 38S
    (10690, "+proj=longlat +ellps=GRS80 +no_defs"), // EUREF-FIN
    (10699, "+proj=utm +zone=34 +ellps=GRS80 +units=m +no_defs"), // EUREF-FIN / UTM zone 34N
    (10702, "+proj=utm +zone=36 +ellps=GRS80 +units=m +no_defs"), // EUREF-FIN / UTM zone 36N
    (10725, "+proj=longlat +ellps=GRS80 +no_defs"), // UZGD2024
    (10726, "+proj=tmerc +lat_0=40 +lon_0=57 +k=0.9999984965 +x_0=350000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // UZGD2024 / UzREF24 zone 40
    (10727, "+proj=tmerc +lat_0=37 +lon_0=63 +k=0.99999730738 +x_0=350000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // UZGD2024 / UzREF24 zone 41
    (10728, "+proj=tmerc +lat_0=36 +lon_0=69 +k=0.9999962402 +x_0=350000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // UZGD2024 / UzREF24 zone 42
    (10729, "+proj=tmerc +lat_0=39 +lon_0=75 +k=0.999995546 +x_0=350000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // UZGD2024 / UzREF24 zone 43
    (10731, "+proj=utm +zone=31 +ellps=GRS80 +units=m +no_defs"), // ETRS89/DREF91/2016 / UTM zone 31N
    (10732, "+proj=utm +zone=32 +ellps=GRS80 +units=m +no_defs"), // ETRS89/DREF91/2016 / UTM zone 32N
    (10733, "+proj=utm +zone=33 +ellps=GRS80 +units=m +no_defs"), // ETRS89/DREF91/2016 / UTM zone 33N
    (10736, "+proj=longlat +ellps=intl +towgs84=1276.2485,-2016.6406,667.4403,101.005288,-212.913401,68.43277,-431.59604 +no_defs"), // Sint Eustatius (Helmert is +exact in PROJ: 2–8 m off here)
    (10739, "+proj=longlat +ellps=GRS80 +no_defs"), // BES2020 Sint Eustatius
    (10744, "+proj=tmerc +lat_0=0 +lon_0=-63 +k=0.9996 +x_0=578.55 +y_0=-1930396.26 +ellps=intl +towgs84=1276.2485,-2016.6406,667.4403,101.005288,-212.913401,68.43277,-431.59604 +units=m +no_defs"), // Sint Eustatius DPnet short (Helmert is +exact in PROJ: 2–8 m off here)
    (10745, "+proj=utm +zone=20 +ellps=intl +towgs84=1276.2485,-2016.6406,667.4403,101.005288,-212.913401,68.43277,-431.59604 +units=m +no_defs"), // Sint Eustatius DPnet long (Helmert is +exact in PROJ: 2–8 m off here)
    (10758, "+proj=longlat +ellps=intl +towgs84=-366.1939,-115.0688,-776.7039,-20.96308,-16.462749,14.276379,-12.809 +no_defs"), // Bonaire
    (10759, "+proj=tmerc +lat_0=12.180658675 +lon_0=-68.2518022805556 +k=1 +x_0=23209.56 +y_0=21423.99 +ellps=intl +towgs84=-366.1939,-115.0688,-776.7039,-20.96308,-16.462749,14.276379,-12.809 +units=m +no_defs"), // Bonaire DPnet
    (10762, "+proj=longlat +ellps=GRS80 +no_defs"), // Bonaire 2004
    (10773, "+proj=tmerc +lat_0=-21.2 +lon_0=-47.7833333333333 +k=1.000092 +x_0=15000 +y_0=25000 +ellps=GRS80 +units=m +no_defs"), // SIRGAS 2000 / Ribeirao Preto Local TM
    (10781, "+proj=longlat +ellps=GRS80 +no_defs"), // ITRF2020-u2023
    (10785, "+proj=longlat +ellps=GRS80 +no_defs"), // IGb20
    (10791, "+proj=longlat +ellps=GRS80 +no_defs"), // UGRF
    (10792, "+proj=utm +zone=35 +ellps=GRS80 +units=m +no_defs"), // UGRF / UTM zone 35N
    (10793, "+proj=utm +zone=36 +ellps=GRS80 +units=m +no_defs"), // UGRF / UTM zone 36N
    (10794, "+proj=utm +zone=36 +south +ellps=GRS80 +units=m +no_defs"), // UGRF / UTM zone 36S
    (10795, "+proj=utm +zone=35 +south +ellps=GRS80 +units=m +no_defs"), // UGRF / UTM zone 35S
    (10800, "+proj=longlat +ellps=GRS80 +no_defs"), // LibRef21
    (10801, "+proj=utm +zone=28 +ellps=GRS80 +units=m +no_defs"), // LibRef21 / UTM zone 28N
    (10802, "+proj=utm +zone=29 +ellps=GRS80 +units=m +no_defs"), // LibRef21 / UTM zone 29N
    (10807, "+proj=longlat +ellps=GRS80 +no_defs"), // NKG_ETRF14
    (10820, "+proj=aea +lat_0=40 +lon_0=-96 +lat_1=44.75 +lat_2=55.75 +x_0=0 +y_0=0 +datum=WGS84 +units=m +no_defs"), // WGS 84 / Agriculture Canada Albers
    (10831, "+proj=longlat +ellps=GRS80 +towgs84=-2.0796,-0.3484,1.7009,-0.05465,0.06718,-0.06143,0.0181 +no_defs"), // Georgia Geodetic Datum
    (10833, "+proj=lcc +lat_0=42.5 +lon_0=43.5 +lat_1=41.6666666666667 +lat_2=42.6666666666667 +x_0=1300000 +y_0=400000 +ellps=GRS80 +towgs84=-2.0796,-0.3484,1.7009,-0.05465,0.06718,-0.06143,0.0181 +units=m +no_defs"), // Georgia Geodetic Datum / Lambert
    (10836, "+proj=utm +zone=37 +ellps=GRS80 +towgs84=-2.0796,-0.3484,1.7009,-0.05465,0.06718,-0.06143,0.0181 +units=m +no_defs"), // Georgia Geodetic Datum / UTM zone 37N (N-E)
    (10837, "+proj=utm +zone=38 +ellps=GRS80 +towgs84=-2.0796,-0.3484,1.7009,-0.05465,0.06718,-0.06143,0.0181 +units=m +no_defs"), // Georgia Geodetic Datum / UTM zone 38N (N-E)
    (10849, "+proj=longlat +ellps=GRS80 +no_defs"), // EWR3-IRF
    (10851, "+proj=tmerc +lat_0=51.95 +lon_0=-0.9 +k=1 +x_0=192519.9715 +y_0=146942.6806 +ellps=GRS80 +units=m +no_defs"), // EWR3 Grid
    (10857, "+proj=aea +lat_0=-12 +lon_0=-54 +lat_1=-2 +lat_2=-22 +x_0=5000000 +y_0=10000000 +ellps=GRS80 +units=m +no_defs"), // SIRGAS 2000 / Brazil Albers
    (10860, "+proj=longlat +ellps=GRS80 +no_defs"), // WSPG-IRF
    (10863, "+proj=tmerc +lat_0=54.5 +lon_0=-0.85 +k=1 +x_0=39630.9213 +y_0=36016.293 +ellps=GRS80 +units=m +no_defs"), // WSPG Grid
    (10875, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRS89-NOR [EUREF89]
    (10892, "+proj=longlat +ellps=GRS80 +no_defs"), // ETRS89-DNK
    (10898, "+proj=longlat +ellps=bessel +no_defs"), // Asse 2025
    (10899, "+proj=tmerc +lat_0=0 +lon_0=12 +k=1 +x_0=4500000 +y_0=0 +ellps=bessel +units=m +no_defs"), // Asse 2025 / 3-degree Gauss-Kruger zone 4 (E-N)
    (10910, "+proj=longlat +ellps=GRS80 +no_defs"), // CSRN epoch 2025.0 (NAD83 2011)
    (10911, "+proj=lcc +lat_0=39.3333333333333 +lon_0=-122 +lat_1=41.6666666666667 +lat_2=40 +x_0=2000000.0001016 +y_0=500000.0001016 +ellps=GRS80 +units=us-ft +no_defs"), // CSRN2025 (NAD83 2011) / California zone 1 (ftUS)
    (10912, "+proj=lcc +lat_0=37.6666666666667 +lon_0=-122 +lat_1=39.8333333333333 +lat_2=38.3333333333333 +x_0=2000000.0001016 +y_0=500000.0001016 +ellps=GRS80 +units=us-ft +no_defs"), // CSRN2025 (NAD83 2011) / California zone 2 (ftUS)
    (10913, "+proj=lcc +lat_0=36.5 +lon_0=-120.5 +lat_1=38.4333333333333 +lat_2=37.0666666666667 +x_0=2000000.0001016 +y_0=500000.0001016 +ellps=GRS80 +units=us-ft +no_defs"), // CSRN2025 (NAD83 2011) / California zone 3 (ftUS)
    (10914, "+proj=lcc +lat_0=35.3333333333333 +lon_0=-119 +lat_1=37.25 +lat_2=36 +x_0=2000000.0001016 +y_0=500000.0001016 +ellps=GRS80 +units=us-ft +no_defs"), // CSRN2025 (NAD83 2011) / California zone 4 (ftUS)
    (10915, "+proj=lcc +lat_0=33.5 +lon_0=-118 +lat_1=35.4666666666667 +lat_2=34.0333333333333 +x_0=2000000.0001016 +y_0=500000.0001016 +ellps=GRS80 +units=us-ft +no_defs"), // CSRN2025 (NAD83 2011) / California zone 5 (ftUS)
    (10916, "+proj=lcc +lat_0=32.1666666666667 +lon_0=-116.25 +lat_1=33.8833333333333 +lat_2=32.7833333333333 +x_0=2000000.0001016 +y_0=500000.0001016 +ellps=GRS80 +units=us-ft +no_defs"), // CSRN2025 (NAD83 2011) / California zone 6 (ftUS)
    (10917, "+proj=aea +lat_0=0 +lon_0=-120 +lat_1=34 +lat_2=40.5 +x_0=0 +y_0=-4000000 +ellps=GRS80 +units=m +no_defs"), // CSRN2025 (NAD83 2011) / California Albers
    (10921, "+proj=lcc +lat_0=39.3333333333333 +lon_0=-122 +lat_1=41.6666666666667 +lat_2=40 +x_0=2000000 +y_0=500000 +ellps=GRS80 +units=m +no_defs"), // CSRN2025 (NAD83 2011) / California zone 1
    (10922, "+proj=lcc +lat_0=37.6666666666667 +lon_0=-122 +lat_1=39.8333333333333 +lat_2=38.3333333333333 +x_0=2000000 +y_0=500000 +ellps=GRS80 +units=m +no_defs"), // CSRN2025 (NAD83 2011) / California zone 2
    (10923, "+proj=lcc +lat_0=36.5 +lon_0=-120.5 +lat_1=38.4333333333333 +lat_2=37.0666666666667 +x_0=2000000 +y_0=500000 +ellps=GRS80 +units=m +no_defs"), // CSRN2025 (NAD83 2011) / California zone 3
    (10924, "+proj=lcc +lat_0=35.3333333333333 +lon_0=-119 +lat_1=37.25 +lat_2=36 +x_0=2000000 +y_0=500000 +ellps=GRS80 +units=m +no_defs"), // CSRN2025 (NAD83 2011) / California zone 4
    (10925, "+proj=lcc +lat_0=33.5 +lon_0=-118 +lat_1=35.4666666666667 +lat_2=34.0333333333333 +x_0=2000000 +y_0=500000 +ellps=GRS80 +units=m +no_defs"), // CSRN2025 (NAD83 2011) / California zone 5
    (10926, "+proj=lcc +lat_0=32.1666666666667 +lon_0=-116.25 +lat_1=33.8833333333333 +lat_2=32.7833333333333 +x_0=2000000 +y_0=500000 +ellps=GRS80 +units=m +no_defs"), // CSRN2025 (NAD83 2011) / California zone 6
    (10941, "+proj=longlat +ellps=GRS80 +no_defs"), // QazTRF-23
    (10942, "+proj=tmerc +lat_0=0 +lon_0=45 +k=1 +x_0=8500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // QazTRF-23 / Gauss-Kruger zone 8
    (10943, "+proj=tmerc +lat_0=0 +lon_0=51 +k=1 +x_0=9500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // QazTRF-23 / Gauss-Kruger zone 9
    (10944, "+proj=tmerc +lat_0=0 +lon_0=57 +k=1 +x_0=10500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // QazTRF-23 / Gauss-Kruger zone 10
    (10945, "+proj=tmerc +lat_0=0 +lon_0=63 +k=1 +x_0=11500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // QazTRF-23 / Gauss-Kruger zone 11
    (10946, "+proj=tmerc +lat_0=0 +lon_0=69 +k=1 +x_0=12500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // QazTRF-23 / Gauss-Kruger zone 12
    (10947, "+proj=tmerc +lat_0=0 +lon_0=75 +k=1 +x_0=13500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // QazTRF-23 / Gauss-Kruger zone 13
    (10948, "+proj=tmerc +lat_0=0 +lon_0=81 +k=1 +x_0=14500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // QazTRF-23 / Gauss-Kruger zone 14
    (10949, "+proj=tmerc +lat_0=0 +lon_0=87 +k=1 +x_0=15500000 +y_0=0 +ellps=GRS80 +units=m +no_defs"), // QazTRF-23 / Gauss-Kruger zone 15
    (10952, "+proj=longlat +ellps=GRS80 +towgs84=1.01575,-1.92011,-0.55977,-0.02778143,0.01178187,-0.01016211,-0.00113124 +no_defs"), // CSRN epoch 2025.0 (ITRF2020)
    (10956, "+proj=longlat +ellps=GRS80 +no_defs"), // GR96(1996)
    (10959, "+proj=longlat +ellps=GRS80 +no_defs"), // GR96(2021)
    (10968, "+proj=longlat +ellps=GRS80 +no_defs"), // NATRF2022
    (10979, "+proj=lcc +lat_1=28 +lat_0=28 +lon_0=-90 +k_0=0.9996 +x_0=1600200 +y_0=533400 +ellps=GRS80 +units=m +no_defs"), // NATRF2022 / Gulf
    (10980, "+proj=lcc +lat_1=28 +lat_0=28 +lon_0=-90 +k_0=0.9996 +x_0=1600200 +y_0=533400 +ellps=GRS80 +units=ft +no_defs"), // NATRF2022 / Gulf (ft)
    (10981, "+proj=lcc +lat_1=30 +lat_0=30 +lon_0=-91.3333333333333 +k_0=0.9999165 +x_0=609610.287 +y_0=147761.325 +ellps=GRS80 +units=ft +no_defs"), // NATRF2022 / Gulf Louisiana Shelf (ft)
    (10982, "+proj=lcc +lat_1=29.3325 +lat_0=29.3325 +lon_0=-99 +k_0=0.9998499 +x_0=609633.909 +y_0=166114.095 +ellps=GRS80 +units=ft +no_defs"), // NATRF2022 / Gulf Texas Shelf North (ft)
    (10983, "+proj=lcc +lat_1=27 +lat_0=27 +lon_0=-98.5 +k_0=0.9998844 +x_0=609630.48 +y_0=147679.41 +ellps=GRS80 +units=ft +no_defs"), // NATRF2022 / Gulf Texas Shelf South (ft)
    (10984, "+proj=tmerc +lat_0=0 +lon_0=-99 +k=0.999584 +x_0=500032.02 +y_0=-161.163 +ellps=GRS80 +units=ft +no_defs"), // NATRF2022 / Gulf West (ft)
    (10985, "+proj=tmerc +lat_0=0 +lon_0=-93 +k=0.9995867 +x_0=500012.589 +y_0=-167.259 +ellps=GRS80 +units=ft +no_defs"), // NATRF2022 / Gulf West Central (ft)
    (10986, "+proj=tmerc +lat_0=0 +lon_0=-87 +k=0.9995904 +x_0=499995.063 +y_0=-178.308 +ellps=GRS80 +units=ft +no_defs"), // NATRF2022 / Gulf East Central (ft)
    (10987, "+proj=tmerc +lat_0=0 +lon_0=-81 +k=0.9995987 +x_0=499980.204 +y_0=-206.121 +ellps=GRS80 +units=ft +no_defs"), // NATRF2022 / Gulf East (ft)
    (11012, "+proj=utm +zone=30 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 30N (N-E)
    (11013, "+proj=utm +zone=31 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 31N (N-E)
    (11014, "+proj=utm +zone=32 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 32N (N-E)
    (11015, "+proj=utm +zone=33 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 33N (N-E)
    (11016, "+proj=utm +zone=34 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 34N (N-E)
    (11017, "+proj=utm +zone=35 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 35N (N-E)
    (11018, "+proj=utm +zone=36 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 36N (N-E)
    (11019, "+proj=utm +zone=37 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 37N (N-E)
    (11020, "+proj=utm +zone=30 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 30N
    (11021, "+proj=utm +zone=31 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 31N
    (11022, "+proj=utm +zone=32 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 32N
    (11023, "+proj=utm +zone=33 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 33N
    (11024, "+proj=utm +zone=34 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 34N
    (11025, "+proj=utm +zone=35 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 35N
    (11026, "+proj=utm +zone=36 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 36N
    (11027, "+proj=utm +zone=37 +ellps=GRS80 +units=m +no_defs"), // ETRS89-NOR [EUREF89] / UTM zone 37N
    (11043, "+proj=longlat +ellps=GRS80 +no_defs"), // CR-SIRGAS epoch 2019.24
    (11114, "+proj=tmerc +lat_0=4.59620322222222 +lon_0=-80.0775077694444 +k=1 +x_0=1000000 +y_0=1000000 +ellps=GRS80 +units=m +no_defs"), // MAGNA-SIRGAS 2018 / Colombia Far West zone
    (11115, "+proj=tmerc +lat_0=4.59620322222222 +lon_0=-77.0775077694444 +k=1 +x_0=1000000 +y_0=1000000 +ellps=GRS80 +units=m +no_defs"), // MAGNA-SIRGAS 2018 / Colombia West zone
    (11116, "+proj=tmerc +lat_0=4.59620322222222 +lon_0=-74.0775077694444 +k=1 +x_0=1000000 +y_0=1000000 +ellps=GRS80 +units=m +no_defs"), // MAGNA-SIRGAS 2018 / Colombia Bogota zone
    (11117, "+proj=tmerc +lat_0=4.59620322222222 +lon_0=-71.0775077694444 +k=1 +x_0=1000000 +y_0=1000000 +ellps=GRS80 +units=m +no_defs"), // MAGNA-SIRGAS 2018 / Colombia East Central zone
    (11118, "+proj=tmerc +lat_0=4.59620322222222 +lon_0=-68.0775077694444 +k=1 +x_0=1000000 +y_0=1000000 +ellps=GRS80 +units=m +no_defs"), // MAGNA-SIRGAS 2018 / Colombia East zone
    (20002, "+proj=tmerc +lat_0=53.35 +lon_0=-2.55 +k=1 +x_0=171975.9382 +y_0=116744.6938 +ellps=GRS80 +units=m +no_defs"), // MWC18 Grid
];
