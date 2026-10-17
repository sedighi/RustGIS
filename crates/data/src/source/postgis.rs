//! PostGIS: the geometry (and geography) tables of a PostgreSQL database, read live in place
//! (desktop only).
//!
//! RustGIS talks to the server itself (the pure-Rust `postgres` client, TLS through rustls), so
//! nothing else needs installing. Extent queries use the column's GiST index (`&&`); the server
//! converts geometry to longitude/latitude (`ST_Transform`, with the server's own PROJ, so every
//! coordinate system in its `spatial_ref_sys` works) and curves to lines. A capped read returns an
//! evenly spread sample, numbered on the server, so only the rows kept cross the network. The
//! connection is read only.
//!
//! What a server sends is hostile input: geometry is decoded by [`wkb`], reads are bounded by
//! `LIMIT`, and statements time out.

use super::{Batch, Family, FeatureSource, Reproject, Request, SourceInfo, clamp_limit, in_request, stride};
use crate::raster::Crs;
use crate::{DataError, Feature, Field, FieldType, GeometryType, Result, Value, wkb};
use postgres::types::ToSql;
use rustgis_geom::Extent;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

/// How long a statement may run on the server.
const STATEMENT_TIMEOUT: &str = "120s";

/// How long connecting may take, when the connection doesn't say.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Tables with an estimated row count above this aren't counted exactly when opened.
const EXACT_COUNT_BELOW: f64 = 2_000_000.0;

/// How long counting or measuring a table when it opens may take (one never analyzed has no
/// estimate, and could be large): past it the count or extent is left unknown.
const PROBE_TIMEOUT: &str = "10s";

/// A geometry (or geography) column of a table or view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub schema: String,
    pub name: String,
    pub column: String,
    /// The SRID the column is declared with (0: unconstrained).
    pub srid: i32,
    /// The declared geometry type (`POINT`, `MULTIPOLYGON`, `GEOMETRY`…).
    pub kind: String,
    pub geography: bool,
    /// Whether the table has other geometry columns (its label then names the column).
    pub several: bool,
}

impl Table {
    /// Its name: with its schema unless that is `public`, and `:column` when it has several.
    pub fn label(&self) -> String {
        let base = if self.schema == "public" { self.name.clone() } else { format!("{}.{}", self.schema, self.name) };
        if self.several { format!("{base}:{}", self.column) } else { base }
    }

    /// Whether `name` (`table`, `schema.table`, either with `:column`, any case) names it.
    pub fn is_named(&self, name: &str) -> bool {
        let (table, column) = match name.rsplit_once(':') {
            Some((t, c)) => (t, Some(c)),
            None => (name, None),
        };
        let full = format!("{}.{}", self.schema, self.name);
        let table_ok = table.eq_ignore_ascii_case(&self.name) || table.eq_ignore_ascii_case(&full);
        table_ok && column.is_none_or(|c| c.eq_ignore_ascii_case(&self.column))
    }

    fn sql_from(&self) -> String {
        format!("{}.{}", ident(&self.schema), ident(&self.name))
    }

    /// The column as a geometry.
    fn geom(&self) -> String {
        if self.geography { format!("{}::geometry", ident(&self.column)) } else { ident(&self.column) }
    }

    /// The geometry families a declared type holds; `None` for `GEOMETRY` (any).
    fn declared_family(&self) -> Option<Family> {
        family_of(&self.kind)
    }
}

/// The family of a PostGIS geometry type name (`MULTIPOLYGON`, `POINTM`, `CURVEPOLYGON`…).
fn family_of(kind: &str) -> Option<Family> {
    let k = kind.to_ascii_uppercase();
    let k = k.strip_prefix("MULTI").unwrap_or(&k);
    if k.starts_with("POINT") {
        Some(Family::Points)
    } else if k.starts_with("CURVEPOLYGON") {
        Some(Family::Polygons)
    } else if k.starts_with("LINESTRING") || k.starts_with("CIRCULARSTRING") || k.starts_with("COMPOUNDCURVE") || k.starts_with("CURVE") {
        Some(Family::Lines)
    } else if k.starts_with("POLYGON")
        || k.starts_with("SURFACE")
        || k.starts_with("TRIANGLE")
        || k.starts_with("TIN")
        || k.starts_with("POLYHEDRALSURFACE")
    {
        Some(Family::Polygons)
    } else {
        None
    }
}

fn geometry_type(f: Option<Family>) -> GeometryType {
    match f {
        Some(Family::Points) => GeometryType::Point,
        Some(Family::Lines) => GeometryType::Polyline,
        Some(Family::Polygons) => GeometryType::Polygon,
        None => GeometryType::None,
    }
}

/// An SQL identifier.
fn ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn err(e: postgres::Error) -> DataError {
    // The server's own message (a missing table, a failed password) rather than "db error".
    match e.as_db_error() {
        Some(db) => DataError::Format(db.message().to_string()),
        None => DataError::Format(e.to_string()),
    }
}

// ---------------------------------------------------------------------------------------------
// Connections

/// How the connection is protected (libpq's `sslmode`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Tls {
    /// No TLS (`disable`).
    Disable,
    /// TLS when the server has it (`prefer`, libpq's default) or always (`require`), without
    /// checking the server's certificate, as libpq does: encrypted, but not proof against a
    /// server in the middle. `verify-full` is.
    Unverified,
    /// TLS with the server's certificate checked (`verify-ca`, `verify-full`): against the
    /// certificates of `sslrootcert` (a private authority), else the web's roots.
    Verified(Option<std::path::PathBuf>),
}

/// A connection to a server, shared by every layer read from it (a database of many tables
/// shouldn't take as many of its connections). Its statements run one at a time: a slow read
/// holds up the other layers of that server until it ends or times out.
pub struct Conn {
    config: postgres::Config,
    tls: Tls,
    client: Mutex<Option<postgres::Client>>,
}

impl Conn {
    /// Runs `f` with the client, connecting again when the connection dropped: a drop (a server
    /// restart, an idle timeout) shows only when it is used, so `f` runs once more on a new one.
    fn with<T>(&self, mut f: impl FnMut(&mut postgres::Client) -> std::result::Result<T, postgres::Error>) -> Result<T> {
        let mut guard = self.client.lock().unwrap_or_else(|e| e.into_inner());
        for attempt in 0..2 {
            if guard.as_ref().is_none_or(postgres::Client::is_closed) {
                *guard = Some(open_client(&self.config, &self.tls)?);
            }
            let client = guard.as_mut().ok_or_else(|| DataError::Format("not connected".into()))?;
            let r = f(client);
            if !client.is_closed() {
                return r.map_err(err);
            }
            *guard = None;
            if attempt == 1 || r.is_ok() {
                return r.map_err(err);
            }
        }
        Err(DataError::Format("not connected".into()))
    }

    /// Runs `f` in a read-only transaction seeing one snapshot of the database (`REPEATABLE
    /// READ`), its statements timing out after `timeout` when given.
    fn snapshot<T>(
        &self,
        timeout: Option<&str>,
        mut f: impl FnMut(&mut postgres::Transaction<'_>) -> std::result::Result<T, postgres::Error>,
    ) -> Result<T> {
        self.with(|c| {
            let mut t = c.build_transaction().isolation_level(postgres::IsolationLevel::RepeatableRead).read_only(true).start()?;
            if let Some(timeout) = timeout {
                t.batch_execute(&format!("SET LOCAL statement_timeout = '{timeout}'"))?;
            }
            let r = f(&mut t)?;
            t.commit()?;
            Ok(r)
        })
    }
}

fn open_client(config: &postgres::Config, tls: &Tls) -> Result<postgres::Client> {
    let mut client = match tls {
        Tls::Disable => config.connect(postgres::NoTls),
        Tls::Unverified | Tls::Verified(_) => config.connect(tokio_postgres_rustls::MakeRustlsConnect::new(rustls_config(tls)?)),
    }
    .map_err(|e| match e.as_db_error() {
        Some(db) => DataError::Format(db.message().to_string()),
        // The cause ("password authentication failed", "connection refused"), not just "error
        // connecting to server".
        None => DataError::Format(std::error::Error::source(&e).map(|s| format!("{e}: {s}")).unwrap_or_else(|| e.to_string())),
    })?;
    client
        .batch_execute(&format!(
            "SET default_transaction_read_only = on; SET statement_timeout = '{STATEMENT_TIMEOUT}'; SET extra_float_digits = 3;"
        ))
        .map_err(err)?;
    Ok(client)
}

fn rustls_config(tls: &Tls) -> Result<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| DataError::Format(format!("TLS: {e}")))?;
    Ok(match tls {
        Tls::Verified(Some(file)) => {
            use rustls::pki_types::{CertificateDer, pem::PemObject};
            let bad = |e: String| DataError::Format(format!("sslrootcert {}: {e}", file.display()));
            let mut roots = rustls::RootCertStore::empty();
            for cert in CertificateDer::pem_file_iter(file).map_err(|e| bad(e.to_string()))? {
                roots.add(cert.map_err(|e| bad(e.to_string()))?).map_err(|e| bad(e.to_string()))?;
            }
            if roots.is_empty() {
                return Err(bad("no certificates".into()));
            }
            builder.with_root_certificates(roots).with_no_client_auth()
        }
        Tls::Verified(None) => {
            let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
            builder.with_root_certificates(roots).with_no_client_auth()
        }
        _ => builder.dangerous().with_custom_certificate_verifier(Arc::new(Unchecked(provider))).with_no_client_auth(),
    })
}

/// libpq's `sslmode=prefer`/`require`: the connection is encrypted, but the server's certificate
/// isn't checked (its signatures are).
#[derive(Debug)]
struct Unchecked(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for Unchecked {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// A live connection: its connection string and the password it was opened with.
type Live = (String, Option<String>, Weak<Conn>);

/// Live connections, so layers of one server share one.
fn live() -> &'static Mutex<Vec<Live>> {
    static L: OnceLock<Mutex<Vec<Live>>> = OnceLock::new();
    L.get_or_init(Default::default)
}

/// Connects to a database (`dsn`: a libpq connection string or `postgresql://` address, as
/// [`super::duckdb::Database::Postgres`] holds it, without its password), or shares a connection
/// already open to it.
///
/// The password is the one remembered for it this session. When `ask` (the user is adding the
/// database), libpq's `PGPASSWORD` and password file serve too; a project opening never hands
/// them to the server it names.
pub fn connect(dsn: &str, ask: bool) -> Result<Arc<Conn>> {
    let (mut config, tls) = config(dsn)?;
    let password = super::duckdb::remembered_password(dsn).or_else(|| {
        if !ask {
            return None;
        }
        std::env::var("PGPASSWORD").ok().filter(|p| !p.is_empty()).or_else(|| pgpass(&config))
    });
    // A connection opened with another password (one given since) isn't shared.
    let find = |live: &mut Vec<Live>| {
        live.retain(|(_, _, w)| w.strong_count() > 0);
        live.iter().find(|(d, p, _)| d == dsn && *p == password).and_then(|(_, _, w)| w.upgrade())
    };
    if let Some(c) = find(&mut live().lock().unwrap_or_else(|e| e.into_inner())) {
        return Ok(c);
    }
    if let Some(p) = &password {
        config.password(p);
    }
    // Connecting can take as long as the timeout: other connections don't wait for it.
    let client = open_client(&config, &tls)?;
    let mut live = live().lock().unwrap_or_else(|e| e.into_inner());
    // Another layer may have connected meanwhile; this one's client then just closes.
    if let Some(c) = find(&mut live) {
        return Ok(c);
    }
    let conn = Arc::new(Conn { config, tls, client: Mutex::new(Some(client)) });
    live.push((dsn.to_string(), password, Arc::downgrade(&conn)));
    Ok(conn)
}

/// A connection string's client configuration, with libpq's defaults (`PGHOST`, `PGPORT`,
/// `PGUSER`, `PGDATABASE`, the user's own name) for what it leaves out.
fn config(dsn: &str) -> Result<(postgres::Config, Tls)> {
    let bad = |e: postgres::Error| DataError::Format(format!("the connection: {e}"));
    let t = dsn.trim();
    let lower = t.to_ascii_lowercase();
    let mut sslmode = None;
    let mut rootcert = None;
    let mut config = if lower.starts_with("postgresql://") || lower.starts_with("postgres://") {
        // `sslmode=verify-…` is libpq's, not the client's: taken out of the query here.
        let (base, query) = t.split_once('?').unwrap_or((t, ""));
        let mut kept = Vec::new();
        for kv in query.split('&').filter(|kv| !kv.is_empty()) {
            match kv.split_once('=') {
                Some((k, v)) if k.eq_ignore_ascii_case("sslmode") => sslmode = Some(v.to_ascii_lowercase()),
                Some((k, v)) if k.eq_ignore_ascii_case("sslrootcert") => rootcert = Some(super::pgconn::percent_decode(v)),
                // libpq options this client has no use for (certificates are checked against the
                // web's roots).
                Some((k, _)) if IGNORED.iter().any(|i| k.eq_ignore_ascii_case(i)) => {}
                _ => kept.push(kv),
            }
        }
        let uri = if kept.is_empty() { base.to_string() } else { format!("{base}?{}", kept.join("&")) };
        uri.parse::<postgres::Config>().map_err(bad)?
    } else {
        let mut c = postgres::Config::new();
        for (k, v) in super::pgconn::keyword_pairs(t) {
            match k.to_ascii_lowercase().as_str() {
                "host" => {
                    for h in v.split(',').filter(|h| !h.is_empty()) {
                        c.host(h);
                    }
                }
                "hostaddr" => {
                    for h in v.split(',').filter_map(|h| h.trim().parse().ok()) {
                        c.hostaddr(h);
                    }
                }
                "port" => {
                    // An empty one is the default port, as in libpq.
                    for p in v.split(',').map(str::trim) {
                        if p.is_empty() {
                            c.port(5432);
                            continue;
                        }
                        c.port(p.parse().map_err(|_| DataError::Format(format!("the connection: port '{p}'")))?);
                    }
                }
                "dbname" => {
                    c.dbname(&v);
                }
                "user" => {
                    c.user(&v);
                }
                "password" => {
                    c.password(v);
                }
                "options" => {
                    c.options(&v);
                }
                "application_name" => {
                    c.application_name(&v);
                }
                "connect_timeout" => {
                    // As libpq: zero or less is no limit of its own, so the default applies.
                    if let Ok(s) = v.trim().parse::<i64>()
                        && s > 0
                    {
                        c.connect_timeout(Duration::from_secs(s.unsigned_abs()));
                    }
                }
                "sslmode" => sslmode = Some(v.to_ascii_lowercase()),
                "sslrootcert" => rootcert = Some(v),
                _ => {}
            }
        }
        c
    };
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if config.get_hosts().is_empty() && config.get_hostaddrs().is_empty() {
        config.host(&env("PGHOST").unwrap_or_else(|| "localhost".into()));
    }
    if config.get_ports().is_empty()
        && let Some(p) = env("PGPORT").and_then(|p| p.parse().ok())
    {
        config.port(p);
    }
    if config.get_user().is_none() {
        let user = env("PGUSER").or_else(|| env("USER")).or_else(|| env("USERNAME")).unwrap_or_else(|| "postgres".into());
        config.user(&user);
    }
    if config.get_dbname().is_none()
        && let Some(d) = env("PGDATABASE")
    {
        config.dbname(&d);
    }
    if config.get_connect_timeout().is_none() {
        config.connect_timeout(CONNECT_TIMEOUT);
    }
    if config.get_application_name().is_none() {
        config.application_name("RustGIS");
    }
    let mode = sslmode.or_else(|| env("PGSSLMODE").map(|m| m.to_ascii_lowercase()));
    let tls = match mode.as_deref() {
        Some("disable") => Tls::Disable,
        // The certificate unchecked, as libpq. For `allow` libpq tries without TLS first, then
        // with it; here TLS first, then without (`prefer`), so a server that only takes TLS
        // connects too.
        Some("allow" | "require" | "prefer") | None => Tls::Unverified,
        // `verify-ca` checks the host name too, stricter than libpq. libpq's
        // `sslrootcert=system` is the system's roots: the web's, here.
        Some("verify-ca") | Some("verify-full") => Tls::Verified(
            rootcert.or_else(|| env("PGSSLROOTCERT")).filter(|r| !r.is_empty() && r != "system").map(std::path::PathBuf::from),
        ),
        Some(other) => return Err(DataError::Format(format!("the connection: sslmode '{other}'"))),
    };
    config.ssl_mode(match (&tls, mode.as_deref()) {
        (Tls::Disable, _) => postgres::config::SslMode::Disable,
        (_, Some("prefer" | "allow") | None) => postgres::config::SslMode::Prefer,
        _ => postgres::config::SslMode::Require,
    });
    Ok((config, tls))
}

/// libpq connection options a `postgresql://` address may carry that this client leaves out.
const IGNORED: &[&str] = &["sslcert", "sslkey", "sslcrl", "gssencmode", "sslcompression", "passfile", "sslpassword"];

/// Where servers usually put their socket: a password file names a connection through one as
/// `localhost`.
#[cfg(unix)]
const DEFAULT_SOCKET_DIRS: &[&str] = &["/var/run/postgresql", "/run/postgresql", "/tmp"];

/// The password for a connection in libpq's password file (`PGPASSFILE`, else `~/.pgpass`, or
/// `%APPDATA%\postgresql\pgpass.conf`), which libpq ignores when others can read it.
fn pgpass(config: &postgres::Config) -> Option<String> {
    let path = std::env::var_os("PGPASSFILE").map(std::path::PathBuf::from).or_else(|| {
        if cfg!(windows) {
            std::env::var_os("APPDATA").map(|a| std::path::Path::new(&a).join("postgresql").join("pgpass.conf"))
        } else {
            std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".pgpass"))
        }
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(&path).ok()?.permissions().mode() & 0o077 != 0 {
            return None;
        }
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let host = pgpass_host(config);
    let port = config.get_ports().first().copied().unwrap_or(5432).to_string();
    let user = config.get_user().unwrap_or_default().to_string();
    let db = config.get_dbname().map(str::to_string).unwrap_or_else(|| user.clone());
    pgpass_find(&text, &[&host, &port, &db, &user])
}

/// The host a password file names a connection by.
fn pgpass_host(config: &postgres::Config) -> String {
    // As libpq: the host, else the address, else `localhost`; a socket in the default directory
    // is `localhost`, another its directory.
    match config.get_hosts().first() {
        Some(postgres::config::Host::Tcp(h)) => h.clone(),
        #[cfg(unix)]
        Some(postgres::config::Host::Unix(dir)) if !DEFAULT_SOCKET_DIRS.iter().any(|d| dir == std::path::Path::new(d)) => {
            dir.display().to_string()
        }
        #[cfg(unix)]
        Some(postgres::config::Host::Unix(_)) => "localhost".into(),
        None => config.get_hostaddrs().first().map_or_else(|| "localhost".into(), ToString::to_string),
    }
}

/// The password of the first line of a password file matching `want` (host, port, database,
/// user; `*` matches anything, `\:` and `\\` escape).
fn pgpass_find(text: &str, want: &[&str; 4]) -> Option<String> {
    for line in text.lines() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        let mut fields = Vec::new();
        let mut cur = String::new();
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => cur.extend(chars.next()),
                ':' if fields.len() < 4 => fields.push(std::mem::take(&mut cur)),
                c => cur.push(c),
            }
        }
        fields.push(cur);
        if fields.len() != 5 {
            continue;
        }
        if fields.iter().zip(want).all(|(f, w)| f == "*" || f == w) {
            return fields.pop();
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Tables

/// The geometry and geography columns of a database's tables and views.
pub fn tables(conn: &Conn) -> Result<Vec<Table>> {
    let sql = "SELECT f_table_schema::text, f_table_name::text, f_geometry_column::text, srid, type::text, false \
               FROM geometry_columns \
               UNION ALL \
               SELECT f_table_schema::text, f_table_name::text, f_geography_column::text, srid, type::text, true \
               FROM geography_columns \
               ORDER BY 1, 2, 3";
    let rows = conn.with(|c| c.query(sql, &[])).map_err(|e| match e {
        DataError::Format(m) if m.contains("geometry_columns") => {
            DataError::Format("the database has no PostGIS (CREATE EXTENSION postgis adds it)".into())
        }
        e => e,
    })?;
    let mut out: Vec<Table> = rows
        .iter()
        .filter_map(|r| {
            Some(Table {
                schema: r.try_get(0).ok()?,
                name: r.try_get(1).ok()?,
                column: r.try_get(2).ok()?,
                srid: r.try_get(3).ok()?,
                kind: r.try_get(4).ok()?,
                geography: r.try_get(5).ok()?,
                several: false,
            })
        })
        .collect();
    let mut columns: std::collections::HashMap<(String, String), usize> = std::collections::HashMap::new();
    for t in &out {
        *columns.entry((t.schema.clone(), t.name.clone())).or_default() += 1;
    }
    for t in &mut out {
        t.several = columns.get(&(t.schema.clone(), t.name.clone())).is_some_and(|n| *n > 1);
    }
    Ok(out)
}

/// The table named `name` (`table`, `schema.table`, `…:column`).
pub fn find_table(conn: &Conn, name: &str) -> Result<Table> {
    let all = tables(conn)?;
    // A name exactly as listed first, then by its parts.
    all.iter()
        .find(|t| t.label().eq_ignore_ascii_case(name))
        .or_else(|| all.iter().find(|t| t.is_named(name)))
        .cloned()
        .ok_or_else(|| DataError::Format(format!("no table '{name}' with a geometry column")))
}

/// The geometry families a table holds: its declared type's, or for a `GEOMETRY` column those its
/// rows have (a layer each).
pub fn families(conn: &Conn, table: &Table) -> Result<Vec<Family>> {
    if let Some(f) = table.declared_family() {
        return Ok(vec![f]);
    }
    let g = table.geom();
    let from = table.sql_from();
    let filter = format!("{g} IS NOT NULL AND NOT ST_IsEmpty({g})");
    let sql = format!("SELECT DISTINCT ST_Dimension({g}) FROM {from} WHERE {filter}");
    // A scan of the table, bounded like the other probes. Past that, the rows may hold any type:
    // a layer each, those without rows left out when they open (by their count). Other errors
    // (no permission, a dropped connection) are the table's.
    let rows = conn.snapshot(Some(PROBE_TIMEOUT), |t| match t.query(sql.as_str(), &[]) {
        Err(e) if e.code() == Some(&postgres::error::SqlState::QUERY_CANCELED) => Ok(None),
        r => r.map(Some),
    })?;
    let Some(rows) = rows else {
        return Ok(vec![Family::Points, Family::Lines, Family::Polygons]);
    };
    let mut out: Vec<Family> = rows
        .iter()
        .filter_map(|r| r.try_get::<_, Option<i32>>(0).ok().flatten())
        .filter_map(|d| match d {
            0 => Some(Family::Points),
            1 => Some(Family::Lines),
            2 | 3 => Some(Family::Polygons),
            _ => None,
        })
        .collect();
    out.sort_by_key(|f| *f as u8);
    out.dedup();
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// The source

/// One geometry column of a table (one family of it, for a column of mixed types), read live.
pub struct PostgisSource {
    info: SourceInfo,
    conn: Arc<Conn>,
    table: Table,
    /// The family read, for a column of mixed types.
    family: Option<Family>,
    /// The SRID the geometry is in (the column's, the rows', or the one given).
    srid: i32,
    /// The SRID the column's values carry (0 when they say none).
    raw_srid: i32,
    /// The geometry, in its own coordinates.
    native: String,
    /// The expression of each column read: the id, the geometry (as WKB, longitude/latitude),
    /// then the fields.
    select: Vec<String>,
    /// Converts a request's extent to the table's coordinates, when RustGIS knows the coordinate
    /// system itself; else the server does.
    reproject: Option<Reproject>,
}

impl std::fmt::Debug for PostgisSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgisSource").field("table", &self.table).field("family", &self.family).finish_non_exhaustive()
    }
}

/// How a column of a PostgreSQL type is read: cast to a type RustGIS has, or as text. `None`
/// leaves it out (other geometry, rasters, binary).
fn field_cast(ty: &str) -> Option<(FieldType, &'static str)> {
    match ty {
        "int2" | "int4" | "int8" | "oid" => Some((FieldType::Integer, "int8")),
        "float4" | "float8" | "numeric" => Some((FieldType::Double, "float8")),
        // `money` has no cast to a number; as text it keeps its currency's format.
        "bool" => Some((FieldType::Boolean, "bool")),
        "geometry" | "geography" | "raster" | "bytea" | "box2d" | "box3d" | "tsvector" => None,
        _ => Some((FieldType::Text, "text")),
    }
}

impl PostgisSource {
    /// Opens a table's geometry column (`family`: the one family to read of a column of mixed
    /// types). `crs` (`EPSG:…`) is the coordinate system of a geometry that doesn't say its own.
    pub fn open(conn: Arc<Conn>, table: Table, family: Option<Family>, crs: Option<&str>) -> Result<Self> {
        let mut notes = Vec::new();
        let g = table.geom();
        // The SRID: the column's, else its rows', else the one given.
        let mut srid = if table.geography && table.srid == 0 { 4326 } else { table.srid };
        if srid == 0 {
            let sql = format!("SELECT ST_SRID({g}) FROM {} WHERE {g} IS NOT NULL LIMIT 1", table.sql_from());
            srid = conn.with(|c| c.query_opt(sql.as_str(), &[]))?.and_then(|r| r.try_get::<_, i32>(0).ok()).unwrap_or(0);
        }
        let raw_srid = srid;
        if table.srid == 0 && !table.geography && raw_srid != 0 {
            notes.push(format!("the column doesn't fix a coordinate system: rows in one other than EPSG:{raw_srid} are left out"));
        }
        let given = crs.map(str::trim).filter(|c| !c.is_empty());
        let mut native = g.clone();
        if srid == 0 {
            match given.map(Crs::parse) {
                Some(Ok(c)) if c.epsg() > 0 => {
                    srid = i32::from(c.epsg());
                    native = format!("ST_SetSRID({g}, {srid})");
                }
                Some(Ok(_)) | Some(Err(_)) => {
                    return Err(DataError::Format(format!("'{}': give the coordinate system as EPSG:<code>", given.unwrap_or_default())));
                }
                None => notes.push(
                    "the geometry doesn't say its coordinate system; read as longitude/latitude (add it again with \"crs\", such as EPSG:2263)".into(),
                ),
            }
        }
        let lonlat = if srid == 0 || srid == 4326 { native.clone() } else { format!("ST_Transform({native}, 4326)") };
        // Curves become lines; Z and M are dropped by the reader.
        let wkb = format!("ST_AsBinary(ST_CurveToLine({lonlat}))");

        // The columns: a single integer primary key gives the ids.
        let rel = format!("{}.{}", ident(&table.schema), ident(&table.name));
        let cols = conn.with(|c| {
            c.query(
                "SELECT a.attname::text, t.typname::text, COALESCE(i.indisprimary, false), c.relkind::text \
                 FROM pg_attribute a \
                 JOIN pg_class c ON c.oid = a.attrelid \
                 JOIN pg_type t ON t.oid = a.atttypid \
                 LEFT JOIN pg_index i ON i.indrelid = a.attrelid AND i.indisprimary AND i.indnatts = 1 AND a.attnum = i.indkey[0] \
                 WHERE a.attrelid = $1::text::regclass AND a.attnum > 0 AND NOT a.attisdropped \
                 ORDER BY a.attnum",
                &[&rel],
            )
        })?;
        let mut fields = Vec::new();
        let mut casts = Vec::new();
        let mut pk = None;
        let mut relkind = String::new();
        for r in &cols {
            let (Ok(name), Ok(ty), Ok(is_pk), Ok(kind)) =
                (r.try_get::<_, String>(0), r.try_get::<_, String>(1), r.try_get::<_, bool>(2), r.try_get::<_, String>(3))
            else {
                continue;
            };
            relkind = kind;
            if is_pk && matches!(ty.as_str(), "int2" | "int4" | "int8") {
                pk = Some(name.clone());
            }
            if name == table.column {
                continue;
            }
            if let Some((ft, cast)) = field_cast(&ty) {
                casts.push(format!("{}::{cast}", ident(&name)));
                fields.push(Field::new(name, ft));
            }
        }
        // Ids stable between reads: the key, else the row's place in the table (`ctid`, which an
        // `UPDATE`, `VACUUM FULL` or `CLUSTER` changes); a view has neither, and a partitioned table's `ctid`s repeat across its partitions, so their
        // rows are numbered as read.
        let id = match (&pk, relkind.as_str()) {
            (Some(k), _) => format!("{}::int8", ident(k)),
            (None, "r" | "m") => "(((ctid::text::point)[0])::int8 * 65536 + ((ctid::text::point)[1])::int8)".to_string(),
            _ => "(row_number() OVER ())::int8".to_string(),
        };
        if pk.is_none() && !matches!(relkind.as_str(), "r" | "m") {
            notes.push("a view or partitioned table without a single-column key: its features are numbered as read, so their ids can change from one read to the next".into());
        } else if pk.is_none() {
            notes.push(
                "no single-column integer key: features are numbered by their place in the table, which changes when rows are updated"
                    .into(),
            );
        }
        let mut select = vec![id, wkb];
        select.extend(casts);

        let family = family.or(table.declared_family());
        let reproject = match srid {
            0 | 4326 => Some(Reproject::from_crs(Crs::LonLat, &mut Vec::new())),
            // A coordinate system RustGIS can't convert itself is left to the server.
            s => u32::try_from(s).ok().and_then(Crs::from_epsg).map(|c| Reproject::from_crs(c, &mut Vec::new())).filter(|r| !r.is_lonlat()),
        };
        let mut src = PostgisSource {
            info: SourceInfo {
                name: table.label(),
                format: "PostGIS",
                geometry_type: geometry_type(family),
                fields,
                extent: Extent::EMPTY,
                count: None,
                crs: if srid == 0 { "EPSG:4326".into() } else { format!("EPSG:{srid}") },
                indexed: false,
                notes,
            },
            conn,
            table,
            family,
            srid,
            raw_srid,
            native,
            select,
            reproject,
        };
        src.info.indexed = src.has_index();
        src.info.count = src.count_rows();
        src.info.extent = src.extent();
        Ok(src)
    }

    /// The table's spatial index on the column, if it has one (a view has none of its own).
    fn has_index(&self) -> bool {
        let rel = format!("{}.{}", ident(&self.table.schema), ident(&self.table.name));
        self.conn
            .with(|c| {
                c.query_opt(
                    "SELECT 1 FROM pg_index i JOIN pg_class x ON x.oid = i.indexrelid JOIN pg_am am ON am.oid = x.relam \
                     JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey) \
                     WHERE i.indrelid = $1::text::regclass AND am.amname IN ('gist', 'spgist', 'brin') AND a.attname = $2 LIMIT 1",
                    &[&rel, &self.table.column],
                )
            })
            .ok()
            .flatten()
            .is_some()
    }

    /// The rows with a geometry (of the family read), exactly unless the table is large.
    fn count_rows(&self) -> Option<u64> {
        let rel = format!("{}.{}", ident(&self.table.schema), ident(&self.table.name));
        let estimate = self
            .conn
            .with(|c| c.query_opt("SELECT reltuples::float8 FROM pg_class WHERE oid = $1::text::regclass", &[&rel]))
            .ok()
            .flatten()
            .and_then(|r| r.try_get::<_, f64>(0).ok());
        match estimate {
            Some(n) if n >= EXACT_COUNT_BELOW => Some(n as u64),
            // No estimate (`-1`: never analyzed) or a small one: counted, within the probe's time.
            _ => {
                let sql = format!("SELECT count(*) FROM {} WHERE {}", self.table.sql_from(), self.filter(false));
                self.conn
                    .snapshot(Some(PROBE_TIMEOUT), |t| t.query_one(sql.as_str(), &[]))
                    .ok()
                    .and_then(|r| r.try_get::<_, i64>(0).ok())
                    .map(|n| n.max(0) as u64)
            }
        }
    }

    /// The table's extent in longitude/latitude: the planner's estimate when it has one, else
    /// measured (only for a table that isn't large).
    fn extent(&self) -> Extent {
        let t = &self.table;
        let estimated = if t.geography {
            None
        } else {
            self.conn
                .with(|c| {
                    c.query_opt(
                        "SELECT ST_XMin(b), ST_YMin(b), ST_XMax(b), ST_YMax(b) FROM (SELECT ST_EstimatedExtent($1, $2, $3) AS b) s WHERE b IS NOT NULL",
                        &[&t.schema, &t.name, &t.column],
                    )
                })
                .ok()
                .flatten()
                .and_then(|r| row_extent(&r))
        };
        let native = estimated.or_else(|| {
            if self.info.count.is_none_or(|n| n as f64 >= EXACT_COUNT_BELOW) {
                return None;
            }
            let g = &self.native;
            let sql = format!(
                "SELECT ST_XMin(b), ST_YMin(b), ST_XMax(b), ST_YMax(b) FROM (SELECT ST_Extent({g}) AS b FROM {} WHERE {}) s WHERE b IS NOT NULL",
                t.sql_from(),
                self.filter(false)
            );
            self.conn.snapshot(Some(PROBE_TIMEOUT), |t| t.query_opt(sql.as_str(), &[])).ok().flatten().and_then(|r| row_extent(&r))
        });
        native.map(|e| self.to_lonlat(&e)).unwrap_or(Extent::EMPTY)
    }

    /// A native extent in longitude/latitude.
    fn to_lonlat(&self, e: &Extent) -> Extent {
        if let Some(r) = &self.reproject {
            return r.extent_to_lonlat(e);
        }
        self.server_transform(e, self.srid, 4326).unwrap_or(Extent::EMPTY)
    }

    /// A longitude/latitude extent in the table's coordinates; `None` when it can't be converted
    /// (the read then filters nothing on the server).
    fn lonlat_to_native(&self, e: &Extent) -> Option<Extent> {
        if let Some(r) = &self.reproject {
            let n = r.extent_from_lonlat(e);
            return (!n.is_empty()).then_some(n);
        }
        let c = Extent::new(e.min_x.max(-180.0), e.min_y.max(-89.9), e.max_x.min(180.0), e.max_y.min(89.9));
        self.server_transform(&c, 4326, self.srid)
    }

    /// An extent converted by the server: its edges, densified, transformed.
    fn server_transform(&self, e: &Extent, from: i32, to: i32) -> Option<Extent> {
        if e.is_empty() {
            return None;
        }
        let step = (e.width().max(e.height()) / 16.0).max(1e-9);
        self.conn
            .with(|c| {
                c.query_opt(
                    "SELECT ST_XMin(b), ST_YMin(b), ST_XMax(b), ST_YMax(b) FROM \
                     (SELECT ST_Transform(ST_Segmentize(ST_MakeEnvelope($1, $2, $3, $4, $5), $6), $7)::box2d AS b) s",
                    &[&e.min_x, &e.min_y, &e.max_x, &e.max_y, &from, &step, &to],
                )
            })
            .ok()
            .flatten()
            .and_then(|r| row_extent(&r))
    }

    /// The `WHERE` condition: a geometry, of the family read, and with `in_extent`, in the extent
    /// given as `$1`–`$4` (in the column's coordinates).
    fn filter(&self, in_extent: bool) -> String {
        let g = self.table.geom();
        let mut w = format!("{g} IS NOT NULL AND NOT ST_IsEmpty({g})");
        // A column of no fixed SRID: only the rows in the one read (others would fail the read).
        if self.table.srid == 0 && !self.table.geography && self.raw_srid != 0 {
            w.push_str(&format!(" AND ST_SRID({g}) = {}", self.raw_srid));
        }
        if in_extent {
            let col = ident(&self.table.column);
            // The column itself, so its index serves (a geography's in longitude/latitude).
            if self.table.geography {
                w.push_str(&format!(" AND {col} && ST_MakeEnvelope($1, $2, $3, $4, 4326)::geography"));
            } else {
                w.push_str(&format!(" AND {col} && ST_MakeEnvelope($1, $2, $3, $4, {})", self.raw_srid));
            }
        }
        if let Some(f) = self.family.filter(|_| self.table.declared_family().is_none()) {
            let dims = match f {
                Family::Points => "0",
                Family::Lines => "1",
                Family::Polygons => "2, 3",
            };
            w.push_str(&format!(" AND ST_Dimension({g}) IN ({dims})"));
        }
        w
    }
}

fn row_extent(r: &postgres::Row) -> Option<Extent> {
    let v: Vec<f64> = (0..4).filter_map(|i| r.try_get::<_, Option<f64>>(i).ok().flatten()).collect();
    match v.as_slice() {
        [a, b, c, d] if v.iter().all(|x| x.is_finite()) => Some(Extent::new(*a, *b, *c, *d)),
        _ => None,
    }
}

impl FeatureSource for PostgisSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&self, req: &Request) -> Result<Batch> {
        let limit = clamp_limit(req.limit);
        let mut out = Batch::default();
        if limit == 0 {
            return Ok(out);
        }
        let native = req.extent.and_then(|e| self.lonlat_to_native(&e));
        let filter = self.filter(native.is_some());
        let args: Vec<f64> = native.map(|e| vec![e.min_x, e.min_y, e.max_x, e.max_y]).unwrap_or_default();
        let params: Vec<&(dyn ToSql + Sync)> = args.iter().map(|a| a as &(dyn ToSql + Sync)).collect();
        let from = self.table.sql_from();
        // Counted first, so a sample keeps every `step`-th row, numbered on the server; both in
        // one snapshot, so the count is of the rows read.
        let (matched, step, rows) = self.conn.snapshot(None, |t| {
            let n: i64 = t.query_one(format!("SELECT count(*) FROM {from} WHERE {filter}").as_str(), &params)?.try_get(0)?;
            let matched = u64::try_from(n).unwrap_or(0);
            let step = stride(matched, limit);
            let sql = if step <= 1 {
                let cols: Vec<String> = self.select.iter().enumerate().map(|(i, e)| format!("{e} AS c{i}")).collect();
                format!("SELECT {} FROM {from} WHERE {filter} LIMIT {limit}", cols.join(", "))
            } else {
                // Rows are numbered as they are, and only those kept are converted (the geometry,
                // the casts): the expressions name the table's columns, which the sample carries.
                let id = self.select.first().map(String::as_str).unwrap_or("0");
                let rest: Vec<String> = self.select.iter().enumerate().skip(1).map(|(i, e)| format!("{e} AS c{i}")).collect();
                format!(
                    "SELECT \"rustgis id\" AS c0, {} FROM (SELECT t.*, {id} AS \"rustgis id\", row_number() OVER () AS \"rustgis rn\" \
                     FROM {from} t WHERE {filter}) s WHERE (\"rustgis rn\" - 1) % {step} = 0 LIMIT {limit}",
                    rest.join(", ")
                )
            };
            Ok((matched, step, t.query(sql.as_str(), &params)?))
        })?;
        out.matched = matched;
        for r in &rows {
            let id = r.try_get::<_, Option<i64>>(0).ok().flatten();
            let geom = r.try_get::<_, Option<Vec<u8>>>(1).ok().flatten().and_then(|b| wkb::decode(&b));
            let (Some(id), Some(geometry)) = (id, geom) else {
                out.skipped += 1;
                continue;
            };
            // The index matched the request's box in the table's coordinates, which holds more
            // than the request when they are projected.
            if !in_request(&geometry, req) {
                continue;
            }
            let attrs = self
                .info
                .fields
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let i = i + 2;
                    match f.ty {
                        FieldType::Integer => r.try_get::<_, Option<i64>>(i).ok().flatten().map(Value::Int),
                        FieldType::Double => r.try_get::<_, Option<f64>>(i).ok().flatten().map(Value::Float),
                        FieldType::Boolean => r.try_get::<_, Option<bool>>(i).ok().flatten().map(Value::Bool),
                        FieldType::Text => r.try_get::<_, Option<String>>(i).ok().flatten().map(Value::Text),
                    }
                    .unwrap_or(Value::Null)
                })
                .collect();
            // A negative key keeps its bits: still one id per row, and the same on every read.
            out.features.push(Feature { id: id.cast_unsigned(), geometry: Some(geometry), attrs });
        }
        out.complete = step <= 1 && (rows.len() as u64) >= out.matched;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_of_declared_types() {
        assert_eq!(family_of("MULTIPOLYGON"), Some(Family::Polygons));
        assert_eq!(family_of("POINTM"), Some(Family::Points));
        assert_eq!(family_of("COMPOUNDCURVE"), Some(Family::Lines));
        assert_eq!(family_of("MULTICURVE"), Some(Family::Lines));
        assert_eq!(family_of("CURVEPOLYGON"), Some(Family::Polygons));
        assert_eq!(family_of("GEOMETRY"), None);
        assert_eq!(family_of("GEOMETRYCOLLECTION"), None);
    }

    #[test]
    fn table_names() {
        let t = |schema: &str, several| Table {
            schema: schema.into(),
            name: "Roads".into(),
            column: "geom".into(),
            srid: 4326,
            kind: "LINESTRING".into(),
            geography: false,
            several,
        };
        assert_eq!(t("public", false).label(), "Roads");
        assert_eq!(t("osm", false).label(), "osm.Roads");
        assert_eq!(t("osm", true).label(), "osm.Roads:geom");
        assert!(t("osm", false).is_named("roads"));
        assert!(t("osm", false).is_named("OSM.roads"));
        assert!(t("osm", true).is_named("osm.roads:GEOM"));
        assert!(!t("osm", true).is_named("osm.roads:other"));
        assert!(!t("osm", false).is_named("other.roads"));
        assert_eq!(ident("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn connection_strings_take_libpq_defaults_and_modes() {
        let (c, tls) = config("host=db.example port=6543 dbname=gis user=me sslmode=verify-full").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(tls, Tls::Verified(None));
        assert_eq!(c.get_ports(), &[6543]);
        assert_eq!(c.get_dbname(), Some("gis"));
        assert_eq!(c.get_user(), Some("me"));
        let (_, tls) = config("host=h dbname=g sslmode=disable").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(tls, Tls::Disable);
        let (c, tls) = config("postgresql://me@h:5433/gis?sslmode=verify-ca&sslrootcert=/x%20y").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(tls, Tls::Verified(Some("/x y".into())));
        let (_, tls) = config("host=h sslmode=verify-full sslrootcert=system").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(tls, Tls::Verified(None));
        // A private authority's file that isn't there fails, rather than falling back.
        let (_, tls) = config("host=h sslmode=verify-full sslrootcert=/nonexistent.pem").unwrap_or_else(|e| panic!("{e}"));
        assert!(rustls_config(&tls).is_err());
        assert_eq!(c.get_ports(), &[5433]);
        assert_eq!(c.get_dbname(), Some("gis"));
        assert!(config("host=h sslmode=bogus").is_err());
        assert!(config("host=h port=x").is_err());
        let (c, _) = config("host=a,b port=,6000").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(c.get_ports(), &[5432, 6000]);
        // A user is always set (libpq's default is the login name).
        let (c, _) = config("host=h").unwrap_or_else(|e| panic!("{e}"));
        assert!(c.get_user().is_some());
        assert!(c.get_connect_timeout().is_some());
        // As libpq, a timeout of zero or less is no limit of its own: the default applies.
        for t in ["0", "-5"] {
            let (c, _) = config(&format!("host=h connect_timeout={t}")).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(c.get_connect_timeout(), Some(&CONNECT_TIMEOUT));
        }
        let (c, _) = config("host=h connect_timeout=30").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(c.get_connect_timeout(), Some(&Duration::from_secs(30)));
    }

    #[test]
    fn password_files() {
        let text = "# comment\nother:5432:*:me:nope\ndb.example:*:gis:me:p\\:w\\\\d\n*:*:*:*:fallback\n";
        assert_eq!(pgpass_find(text, &["db.example", "5432", "gis", "me"]).as_deref(), Some("p:w\\d"));
        assert_eq!(pgpass_find(text, &["x", "1", "y", "z"]).as_deref(), Some("fallback"));
        assert_eq!(pgpass_find("a:b:c", &["a", "b", "c", "d"]), None);
        // The host it names: the host, else the address, else `localhost`.
        let host = |dsn: &str| pgpass_host(&config(dsn).unwrap_or_else(|e| panic!("{e}")).0);
        assert_eq!(host("host=db.example hostaddr=10.0.0.1"), "db.example");
        assert_eq!(host("hostaddr=10.0.0.1"), "10.0.0.1");
        #[cfg(unix)]
        {
            assert_eq!(host("host=/var/run/postgresql"), "localhost");
            assert_eq!(host("host=/srv/pg"), "/srv/pg");
        }
    }

    #[test]
    fn field_casts() {
        assert_eq!(field_cast("int4"), Some((FieldType::Integer, "int8")));
        assert_eq!(field_cast("numeric"), Some((FieldType::Double, "float8")));
        assert_eq!(field_cast("timestamptz"), Some((FieldType::Text, "text")));
        assert_eq!(field_cast("geometry"), None);
    }

    /// Reads every table of a live database: `RUSTGIS_TEST_POSTGIS="host=… dbname=… user=…
    /// password=…" cargo test -p rustgis-data -- --ignored postgis_live --nocapture`.
    #[test]
    #[ignore]
    fn postgis_live() {
        let Ok(dsn) = std::env::var("RUSTGIS_TEST_POSTGIS") else { return };
        let db = super::super::duckdb::Database::parse(&dsn);
        let super::super::duckdb::Database::Postgres(dsn) = db else { panic!("not a connection") };
        let conn = connect(&dsn, true).unwrap_or_else(|e| panic!("{e}"));
        // A connection the server drops is opened again on the next use, which still works.
        let _ = conn.with(|c| c.batch_execute("SELECT pg_terminate_backend(pg_backend_pid())"));
        let all = tables(&conn).unwrap_or_else(|e| panic!("after a dropped connection: {e}"));
        for t in all.iter().filter(|t| t.schema != "tiger") {
            let fams = families(&conn, t).unwrap_or_else(|e| panic!("{}: {e}", t.label()));
            for f in fams.iter().copied().map(Some).chain(fams.is_empty().then_some(None)) {
                let start = std::time::Instant::now();
                let src = match PostgisSource::open(conn.clone(), t.clone(), f, None) {
                    Ok(s) => s,
                    Err(e) => {
                        println!("{:<28} open failed: {e}", t.label());
                        continue;
                    }
                };
                let opened = start.elapsed();
                let i = src.info();
                let all = src.read(&Request { extent: None, limit: 250_000 }).unwrap_or_else(|e| panic!("{}: {e}", t.label()));
                let read = start.elapsed() - opened;
                // A view of a quarter of the extent.
                let e = i.extent;
                let q = Extent::new(e.min_x, e.min_y, e.min_x + e.width() / 2.0, e.min_y + e.height() / 2.0);
                let part = src.read(&Request { extent: Some(q), limit: 1000 }).unwrap_or_else(|e| panic!("{}: {e}", t.label()));
                for ft in &part.features {
                    let g = ft.geometry.as_ref().map(|g| g.extent()).unwrap_or(Extent::EMPTY);
                    assert!(g.intersects(&q.padded(0.01, 0.0)), "{}: feature {} outside the request", t.label(), ft.id);
                }
                let mut ids: Vec<u64> = all.features.iter().map(|f| f.id).collect();
                ids.sort_unstable();
                ids.dedup();
                assert_eq!(ids.len(), all.features.len(), "{}: ids repeat", t.label());
                println!(
                    "{:<28} {:<9} {:?} {:>9} rows ({:?}) idx={} crs={} extent=[{:.2} {:.2} {:.2} {:.2}] fields={} all: {} of {} complete={} skipped={} | quarter: {} of {} | open {:?} read {:?} notes={:?}",
                    t.label(),
                    f.map(|f| f.label()).unwrap_or("-"),
                    i.geometry_type,
                    i.count.unwrap_or(0),
                    t.kind,
                    i.indexed,
                    i.crs,
                    e.min_x,
                    e.min_y,
                    e.max_x,
                    e.max_y,
                    i.fields.len(),
                    all.features.len(),
                    all.matched,
                    all.complete,
                    all.skipped,
                    part.features.len(),
                    part.matched,
                    opened,
                    read,
                    i.notes
                );
            }
        }
    }
}
