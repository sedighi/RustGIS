//! DuckDB, optionally: SQL queries (with DuckDB Spatial's `ST_` functions, over files, web
//! addresses and the tables of a DuckDB database) and the tables of DuckDB database files, run
//! through the `duckdb` program when it is installed.
//!
//! A query's result is written as GeoParquet in the cache folder, in longitude/latitude, sorted
//! along a Hilbert curve and with a bounding-box covering, so the GeoParquet provider reads a large
//! one a view at a time. A result mixing geometry types becomes one file per type. Nothing links to
//! DuckDB: without the program, queries report how to get it.
//!
//! The SQL reaches DuckDB only as a string literal handed to its `query()` table function, which
//! runs a single SELECT: it can't end the statement, run other statements (`COPY`, `ATTACH`,
//! `INSTALL`) or the shell's dot commands. It can still read any file and web address, as it is
//! meant to: the AI Assistant asks before running a query, and plugins need both the files and
//! network permissions (`rustgis_engine::cmd::plugins::SQL`).

use super::Family;
use super::external::{self, TempFile, fnv1a, output, stamp, touch};
use crate::{DataError, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// DuckDB database files.
pub const EXTENSIONS: &[&str] = &["duckdb", "ddb"];

/// Longest SQL accepted (the command line it travels on is limited, on Windows to 32K characters).
pub const MAX_SQL: usize = 12_000;

/// Most tables of a database added at once.
pub const MAX_TABLES: usize = 64;

/// Most tables of a database listed.
const LIST_LIMIT: usize = 10_000;

/// The longest a query may run, and the longest a table listing may.
const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);
const LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Results not used for this long are removed from the cache.
const KEEP: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);

/// The column added to sort rows into geometry families; never in a result.
const DIM: &str = "__rustgis_dim";

/// A database DuckDB reads: a DuckDB file, or a PostgreSQL database (PostGIS geometry included)
/// through DuckDB's `postgres` extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Database {
    File(PathBuf),
    /// A libpq connection string (`host=… dbname=… user=…`) or `postgresql://` URI, without its
    /// password (see [`remember_password`]).
    Postgres(String),
}

impl Database {
    /// A database by how projects and commands name it: `postgres:<connection string>`, a
    /// `postgresql://` URI, or a file path. A password in it is taken out and remembered for the
    /// session.
    pub fn parse(s: &str) -> Database {
        let t = s.trim();
        let lower = t.to_ascii_lowercase();
        let dsn = if let Some(rest) =
            t.get(..9).filter(|p| p.eq_ignore_ascii_case("postgres:")).and_then(|_| t.get(9..)).filter(|r| !r.starts_with("//"))
        {
            Some(rest.trim().to_string())
        } else if lower.starts_with("postgresql://") || lower.starts_with("postgres://") {
            Some(t.to_string())
        } else {
            None
        };
        match dsn {
            Some(d) => {
                let (public, password) = split_password(&d);
                if let Some(p) = password {
                    remember_password(&public, &p);
                }
                Database::Postgres(public)
            }
            None => Database::File(PathBuf::from(t)),
        }
    }

    /// How projects save it (never with a password).
    pub fn label(&self) -> String {
        match self {
            Database::File(p) => p.display().to_string(),
            Database::Postgres(d) if d.contains("://") => d.clone(),
            Database::Postgres(d) => format!("postgres:{d}"),
        }
    }

    /// What a layer's name and messages call it: the file, or the database and host.
    pub fn short(&self) -> String {
        match self {
            Database::File(p) => p.file_stem().and_then(|s| s.to_str()).unwrap_or("database").to_string(),
            Database::Postgres(d) => {
                let get = |k: &str| d.split_whitespace().find_map(|kv| kv.strip_prefix(&format!("{k}=")).map(str::to_string));
                get("dbname")
                    .unwrap_or_else(|| d.rsplit('/').next().unwrap_or("postgres").split('?').next().unwrap_or("postgres").to_string())
            }
        }
    }
}

/// A connection string without its password, and the password.
fn split_password(dsn: &str) -> (String, Option<String>) {
    super::pgconn::split_secrets(dsn, &["password"])
}

pub use super::pgconn::public_label;

fn passwords() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static P: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();
    P.get_or_init(Default::default)
}

/// Remembers a connection's password for this session (never saved: projects name connections
/// without them, and libpq's `PGPASSWORD` or `~/.pgpass` serve when it isn't remembered).
pub fn remember_password(dsn: &str, password: &str) {
    passwords().lock().unwrap_or_else(|e| e.into_inner()).insert(dsn.to_string(), password.to_string());
}

/// The password remembered for a connection this session, if any.
pub(crate) fn remembered_password(dsn: &str) -> Option<String> {
    passwords().lock().unwrap_or_else(|e| e.into_inner()).get(dsn).cloned()
}

/// The remembered password of a query's database, handed to DuckDB's PostgreSQL client in its
/// environment (`PGPASSWORD`): never in the script, where the process list and DuckDB's errors
/// would show it.
fn password(database: Option<&Database>) -> Option<String> {
    match database? {
        Database::Postgres(dsn) => passwords().lock().unwrap_or_else(|e| e.into_inner()).get(dsn).cloned(),
        Database::File(_) => None,
    }
}

/// The error of a query (or table) without rows.
pub const NO_ROWS: &str = "the query returned no rows";

/// Whether a file name is a DuckDB database.
pub fn is_duckdb_name(name: &str) -> bool {
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    EXTENSIONS.contains(&ext.as_str())
}

/// The `duckdb` program: in the folder `RUSTGIS_DUCKDB_BIN` names, else on the `PATH`.
pub fn program() -> Option<PathBuf> {
    external::program("RUSTGIS_DUCKDB_BIN", "duckdb")
}

/// Whether DuckDB is installed.
pub fn available() -> bool {
    program().is_some()
}

fn missing() -> DataError {
    DataError::Unsupported(
        "SQL queries and DuckDB databases need DuckDB: install it (the duckdb program on the PATH, or RUSTGIS_DUCKDB_BIN naming its folder) and try again"
            .into(),
    )
}

/// Where results are kept: the platform's cache folder, or `RUSTGIS_DUCKDB_CACHE`.
pub fn cache_dir() -> Option<PathBuf> {
    external::cache_dir("RUSTGIS_DUCKDB_CACHE", "duckdb")
}

/// A query to run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query<'a> {
    /// One SELECT statement (or `FROM …`, `WITH …`, `VALUES …`).
    pub sql: &'a str,
    /// A database opened (read only) for the query, so its tables are named plainly.
    pub database: Option<&'a Database>,
    /// The coordinate system of a geometry column that doesn't say its own (`EPSG:2263`).
    pub crs: Option<&'a str>,
}

/// One geometry family of a result, as GeoParquet.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub path: PathBuf,
    pub family: Family,
    pub count: u64,
}

/// A result: a part per geometry family it holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub parts: Vec<Part>,
    /// Rows without a geometry, left out.
    pub skipped: u64,
}

/// A table of a database with a geometry column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub schema: String,
    pub name: String,
    pub geom_col: Option<String>,
    pub geom_type: Option<String>,
    pub feature_dataset: Option<String>,
    pub srid: Option<i32>,
}

impl Table {
    pub fn new(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self { schema: schema.into(), name: name.into(), geom_col: None, geom_type: None, feature_dataset: None, srid: None }
    }

    /// The detected coordinate system as an EPSG string (e.g. "EPSG:32639").
    pub fn crs(&self) -> Option<String> {
        self.srid.filter(|&s| s > 0).map(|s| format!("EPSG:{s}"))
    }

    /// Its name, with its schema unless that is `main`.
    pub fn label(&self) -> String {
        if self.schema == "main" { self.name.clone() } else { format!("{}.{}", self.schema, self.name) }
    }

    /// Whether this table uses Esri SDE geometry (e.g. sde.st_geometry).
    pub fn is_sde(&self) -> bool {
        self.geom_type
            .as_deref()
            .map(|t| {
                let lower = t.to_ascii_lowercase();
                lower.contains("sde") || lower.contains("st_geometry")
            })
            .unwrap_or(false)
            || self.schema.eq_ignore_ascii_case("sde")
    }

    pub fn sql(&self) -> String {
        if self.is_sde() {
            let col = self.geom_col.as_deref().unwrap_or("shape");
            let pg_sql = if col.eq_ignore_ascii_case("geometry") {
                format!("SELECT *, sde.st_astext({}) AS __rgis_wkt FROM {}.{}", ident(col), ident(&self.schema), ident(&self.name))
            } else {
                format!("SELECT *, sde.st_astext({}) AS geometry FROM {}.{}", ident(col), ident(&self.schema), ident(&self.name))
            };
            if col.eq_ignore_ascii_case("geometry") {
                format!(
                    "FROM (SELECT * EXCLUDE (geometry, __rgis_wkt), ST_GeomFromText(__rgis_wkt) AS geometry FROM postgres_query('src', {}))",
                    literal(&pg_sql)
                )
            } else {
                format!(
                    "FROM (SELECT * EXCLUDE (geometry), ST_GeomFromText(geometry) AS geometry FROM postgres_query('src', {}))",
                    literal(&pg_sql)
                )
            }
        } else {
            format!("FROM {}.{}", ident(&self.schema), ident(&self.name))
        }
    }
}

/// Wraps a query containing Esri `sde.st_astext` or `sde.st_asbinary` to run through DuckDB's
/// `postgres_query` and convert the result into a native DuckDB `geometry` column.
pub fn wrap_sde_query_if_needed(sql: &str) -> String {
    let lower = sql.to_ascii_lowercase();
    if (lower.contains("sde.st_astext") || lower.contains("sde.st_asbinary")) && !lower.contains("postgres_query") {
        let is_binary = lower.contains("sde.st_asbinary");
        let geom_col = if lower.contains(" as geom,") || lower.ends_with(" as geom") {
            "geom"
        } else if lower.contains(" as shape,") || lower.ends_with(" as shape") {
            "shape"
        } else {
            "geometry"
        };
        if is_binary {
            format!("SELECT * EXCLUDE ({geom_col}), ST_GeomFromWKB({geom_col}) AS geometry FROM postgres_query('src', {})", literal(sql))
        } else {
            format!("SELECT * EXCLUDE ({geom_col}), ST_GeomFromText({geom_col}) AS geometry FROM postgres_query('src', {})", literal(sql))
        }
    } else {
        sql.to_string()
    }
}

/// An SQL string literal.
fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// An SQL identifier.
fn ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Checks a coordinate system given by name.
fn check_crs(crs: Option<&str>) -> Result<()> {
    match crs {
        Some(c) if c.is_empty() || c.len() > 64 || !c.chars().all(|ch| ch.is_ascii_alphanumeric() || ":_.".contains(ch)) => {
            Err(DataError::Format(format!("crs '{c}': give a code such as EPSG:2263")))
        }
        _ => Ok(()),
    }
}

/// The statements every run starts with: DuckDB Spatial (installed the first time), x/y axis
/// order whatever the coordinate system, and the database, when there is one.
///
/// `confined` shuts DuckDB out of every file and web address but the cache folder once the
/// database is open: a database file is hostile input, and its views can hold any SQL. Tables are
/// read that way, so adding a database (or a project naming one) reads nothing else.
fn prelude(dir: &Path, database: Option<&Database>, confined: bool) -> Result<String> {
    let tmp = dir.join("spill");
    let mut s = format!(
        "INSTALL spatial; LOAD spatial; SET geometry_always_xy = true; SET enable_progress_bar = false; SET temp_directory = {};",
        literal(&tmp.display().to_string())
    );
    match database {
        Some(Database::File(db)) => {
            let full = std::path::absolute(db).unwrap_or_else(|_| db.to_path_buf());
            if !full.is_file() {
                return Err(DataError::Format(format!("{}: no such database", db.display())));
            }
            s.push_str(&format!(" ATTACH {} AS src (READ_ONLY); USE src;", literal(&full.display().to_string())));
        }
        // PostGIS geometry columns come through as DuckDB geometries with their coordinate system.
        Some(Database::Postgres(dsn)) => {
            s.push_str(&format!(" INSTALL postgres; LOAD postgres; ATTACH {} AS src (TYPE postgres, READ_ONLY); USE src;", literal(dsn)));
        }
        None => {}
    }
    if confined {
        let allowed = format!("{}{}", dir.display(), std::path::MAIN_SEPARATOR);
        s.push_str(&format!(
            " SET allowed_directories = [{}]; SET enable_external_access = false; SET lock_configuration = true;",
            literal(&allowed)
        ));
    }
    Ok(s)
}

/// Runs a script.
fn run_script(dir: &Path, script: &str, database: Option<&Database>, timeout: std::time::Duration) -> Result<()> {
    let duckdb = program().ok_or_else(missing)?;
    // An empty start-up file, so the user's own (~/.duckdbrc) can't change what runs.
    let init = dir.join("init.sql");
    if !init.is_file() {
        std::fs::write(&init, b"")?;
    }
    let mut c = Command::new(duckdb);
    c.arg("-init").arg(&init).arg("-c").arg(script);
    if let Some(p) = password(database) {
        c.env("PGPASSWORD", p);
    }
    let out = output(c, timeout, "DuckDB")?;
    if !out.status.success() {
        return Err(DataError::Format(format!("DuckDB: {}", message(&String::from_utf8_lossy(&out.stderr), &out.status))));
    }
    Ok(())
}

/// Runs `script`, then `select`, whose rows DuckDB writes to a file that is read back: not taken
/// from what it prints, which warnings share.
fn fetch(dir: &Path, script: &str, database: Option<&Database>, select: &str, timeout: std::time::Duration) -> Result<serde_json::Value> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file = dir.join(format!("{:016x}.{}.{n}.rows.tmp.json", fnv1a(select.as_bytes()), std::process::id()));
    let _guard = TempFile(Some(file.clone()));
    let copy = format!("{script} COPY ({select}) TO {} (FORMAT json, ARRAY true);", literal(&file.display().to_string()));
    run_script(dir, &copy, database, timeout)?;
    serde_json::from_slice(&std::fs::read(&file)?).map_err(|e| DataError::Format(format!("DuckDB: unreadable result ({e})")))
}

/// What went wrong, from DuckDB's error output: its first lines, without colours, warnings or
/// the excerpt of the statement (which shows the query wrapped, not as it was written).
fn message(stderr: &str, status: &std::process::ExitStatus) -> String {
    let plain = strip_ansi(stderr);
    let lines: Vec<&str> = plain.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    // An error is reported after any warnings.
    let from = lines.iter().position(|l| l.contains("Error")).unwrap_or(0);
    let msg: Vec<&str> = lines.iter().skip(from).take_while(|l| !l.starts_with("LINE ")).take(3).copied().collect();
    if msg.is_empty() { format!("duckdb failed ({status})") } else { msg.join(" ") }
}

/// Text without terminal colour codes.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC [ … letter
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A column of a result.
#[derive(Clone, Debug, PartialEq)]
struct Column {
    name: String,
    ty: String,
}

/// Column types the GeoParquet provider reads as they are; others are written as text.
fn kept_as_is(ty: &str) -> bool {
    matches!(
        ty,
        "BOOLEAN" | "TINYINT" | "SMALLINT" | "INTEGER" | "BIGINT" | "UTINYINT" | "USMALLINT" | "UINTEGER" | "FLOAT" | "DOUBLE" | "VARCHAR"
    ) || ty.starts_with("DECIMAL(")
}

fn is_geometry(ty: &str) -> bool {
    ty == "GEOMETRY" || ty.starts_with("GEOMETRY(")
}

/// The coordinate system a geometry type says (`GEOMETRY('EPSG:3857')`).
fn type_crs(ty: &str) -> Option<&str> {
    ty.strip_prefix("GEOMETRY('").and_then(|r| r.strip_suffix("')")).filter(|c| !c.is_empty())
}

/// The SELECT that turns a result into what is written: the first geometry column in
/// longitude/latitude, other geometry columns as WKT, types the provider doesn't read as text, and
/// each row's geometry family; and the geometry column's name.
fn select(columns: &[Column], crs: Option<&str>, from: &str) -> Result<(String, String)> {
    let geom = columns
        .iter()
        .find(|c| is_geometry(&c.ty))
        .ok_or_else(|| DataError::Format("the query returns no geometry column: select one (such as ST_Point(lon, lat) AS geom)".into()))?;
    let g = ident(&geom.name);
    // A geometry that says no coordinate system (the usual case for longitude/latitude) is taken
    // as it is.
    let geom_expr = match (crs, type_crs(&geom.ty)) {
        (Some(c), _) => format!("ST_Transform({g}::GEOMETRY, {}, 'EPSG:4326')", literal(c)),
        (None, Some(t)) if t.eq_ignore_ascii_case("EPSG:4326") || t.eq_ignore_ascii_case("OGC:CRS84") => g.clone(),
        (None, Some(_)) => format!("ST_Transform({g}, 'EPSG:4326')"),
        (None, None) => g.clone(),
    };
    let list: Vec<String> = columns
        .iter()
        .map(|c| {
            let n = ident(&c.name);
            if std::ptr::eq(c, geom) {
                format!("ST_Force2D({geom_expr}) AS {n}")
            } else if is_geometry(&c.ty) {
                format!("ST_AsText({n}) AS {n}")
            } else if kept_as_is(&c.ty) {
                n
            } else {
                format!("CAST({n} AS VARCHAR) AS {n}")
            }
        })
        .collect();
    Ok((format!("SELECT *, ST_Dimension({g}) AS {DIM} FROM (SELECT {} FROM {from})", list.join(", ")), geom.name.clone()))
}

/// Runs a query, writing its result into the cache (always afresh: the data it reads may have
/// changed).
pub fn run(q: &Query<'_>) -> Result<Output> {
    let dir = cache_dir().ok_or_else(|| DataError::Format("no cache folder for DuckDB results".into()))?;
    run_in(&dir, q, false)
}

/// The most rows a preview returns.
pub const MAX_PREVIEW: usize = 1000;
/// The longest value a preview shows (text and geometry as WKT are cut there).
const PREVIEW_TEXT: usize = 300;

/// A query's first rows, as text: what an SQL workspace shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Preview {
    /// Each column's name and DuckDB type.
    pub columns: Vec<(String, String)>,
    /// The rows, each value as JSON (numbers, booleans and text; null; geometries as WKT).
    pub rows: Vec<Vec<serde_json::Value>>,
    /// Whether the result has a geometry column (it can be added to the map).
    pub geometry: bool,
    /// Whether there are rows past those returned.
    pub more: bool,
}

/// Checks a query before it runs: one statement, not empty, not too long.
fn checked(sql: &str) -> Result<&str> {
    let sql = sql.trim().trim_end_matches(';').trim_end();
    if sql.is_empty() {
        return Err(DataError::Format("the query is empty".into()));
    }
    if sql.len() > MAX_SQL {
        return Err(DataError::Format(format!("the query is longer than {MAX_SQL} characters")));
    }
    if sql.contains('\0') {
        return Err(DataError::Format("the query holds a NUL character".into()));
    }
    Ok(sql)
}

/// Runs a query and returns its first `limit` rows (at most [`MAX_PREVIEW`]), without writing a
/// result: an SQL workspace's preview. Geometries come back as WKT, other values DuckDB doesn't
/// give as JSON scalars as text, and long values cut.
pub fn preview(q: &Query<'_>, limit: usize) -> Result<Preview> {
    let dir = cache_dir().ok_or_else(|| DataError::Format("no cache folder for DuckDB results".into()))?;
    preview_in(&dir, q, limit)
}

fn preview_in(dir: &Path, q: &Query<'_>, limit: usize) -> Result<Preview> {
    let sql = checked(q.sql)?;
    if !available() {
        return Err(missing());
    }
    std::fs::create_dir_all(dir)?;
    let limit = limit.clamp(1, MAX_PREVIEW);
    let prelude = prelude(dir, q.database, false)?;
    let from = format!("query({})", literal(sql));
    let described =
        fetch(dir, &prelude, q.database, &format!("SELECT column_name, column_type FROM (DESCRIBE FROM {from})"), QUERY_TIMEOUT)?;
    let columns: Vec<Column> = described
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| Some(Column { name: c.get("column_name")?.as_str()?.to_string(), ty: c.get("column_type")?.as_str()?.to_string() }))
        .take(500)
        .collect();
    if columns.is_empty() {
        return Err(DataError::Format("the query returns no columns".into()));
    }
    // Columns by position (`c0`, `c1`…), so names that repeat stay apart.
    let list: Vec<String> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let n = ident(&c.name);
            let expr = if is_geometry(&c.ty) {
                format!("left(ST_AsText({n}), {PREVIEW_TEXT})")
            } else if matches!(c.ty.as_str(), "FLOAT" | "DOUBLE") {
                // NaN and infinities aren't JSON numbers: shown empty.
                format!("CASE WHEN isfinite({n}) THEN {n} END")
            } else if matches!(
                c.ty.as_str(),
                "BOOLEAN" | "TINYINT" | "SMALLINT" | "INTEGER" | "BIGINT" | "UTINYINT" | "USMALLINT" | "UINTEGER"
            ) {
                n
            } else {
                format!("left(CAST({n} AS VARCHAR), {PREVIEW_TEXT})")
            };
            format!("{expr} AS c{i}")
        })
        .collect();
    // The columns come from DESCRIBE, run against the query itself.
    let select = format!("SELECT {} FROM {from} LIMIT {}", list.join(", "), limit + 1);
    let fetched = fetch(dir, &prelude, q.database, &select, QUERY_TIMEOUT)?;
    let mut rows: Vec<Vec<serde_json::Value>> = fetched
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| (0..columns.len()).map(|i| r.get(format!("c{i}")).cloned().unwrap_or(serde_json::Value::Null)).collect())
        .collect();
    let more = rows.len() > limit;
    rows.truncate(limit);
    Ok(Preview {
        geometry: columns.iter().any(|c| is_geometry(&c.ty)),
        columns: columns.into_iter().map(|c| (c.name, c.ty)).collect(),
        rows,
        more,
    })
}

/// The tables of a database and their columns (`table` or `schema.table`, column), for completing
/// names in an SQL workspace. Read confined, as [`tables_of`] is.
pub fn schema_of(database: &Database) -> Result<Vec<(String, String)>> {
    let dir = cache_dir().ok_or_else(|| DataError::Format("no cache folder for DuckDB results".into()))?;
    if !available() {
        return Err(missing());
    }
    std::fs::create_dir_all(&dir)?;
    let select = format!(
        "SELECT schema_name, table_name, column_name FROM duckdb_columns() WHERE database_name = 'src' ORDER BY schema_name, table_name, column_index LIMIT {LIST_LIMIT}"
    );
    let rows = fetch(&dir, &prelude(&dir, Some(database), true)?, Some(database), &select, LIST_TIMEOUT)?;
    Ok(rows
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let t = Table::new(r.get("schema_name")?.as_str()?, r.get("table_name")?.as_str()?);
            Some((t.label(), r.get("column_name")?.as_str()?.to_string()))
        })
        .collect())
}

/// Runs a query; `confined` (for a table of a database) as [`prelude`] says. The result's parts are
/// named after this run alone, so a run never replaces a file a layer may be reading.
fn run_in(dir: &Path, q: &Query<'_>, confined: bool) -> Result<Output> {
    let sql = checked(q.sql)?;
    check_crs(q.crs)?;
    if !available() {
        return Err(missing());
    }
    std::fs::create_dir_all(dir)?;
    external::prune(dir, ours, KEEP);
    let prelude = prelude(dir, q.database, confined)?;
    let from = format!("query({})", literal(sql));

    // The result's columns, from DuckDB binding the query (not running it).
    let described =
        fetch(dir, &prelude, q.database, &format!("SELECT column_name, column_type FROM (DESCRIBE FROM {from})"), QUERY_TIMEOUT)?;
    let columns: Vec<Column> = described
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| Some(Column { name: c.get("column_name")?.as_str()?.to_string(), ty: c.get("column_type")?.as_str()?.to_string() }))
        .collect();
    if columns.iter().any(|c| c.name == DIM) {
        return Err(DataError::Format(format!("the query returns a column named {DIM}")));
    }
    let (select, geom) = select(&columns, q.crs, &from)?;
    let g = ident(&geom);
    // The bounding-box covering, unless the result already has a column of its name.
    let bbox_name = format!("{geom}_bbox");
    let bbox = if columns.iter().any(|c| c.name.eq_ignore_ascii_case(&bbox_name)) {
        String::new()
    } else {
        format!(
            ", struct_pack(xmin := ST_XMin({g}), ymin := ST_YMin({g}), xmax := ST_XMax({g}), ymax := ST_YMax({g})) AS {}",
            ident(&bbox_name)
        )
    };

    // Written under a name of its own, so a failed run leaves nothing behind and runs don't meet.
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let run_tag = format!("{:016x}", fnv1a(format!("{sql}\n{:?}\n{:?}\n{now}\n{}\n{n}", q.database, q.crs, std::process::id()).as_bytes()));
    let tmp = |f: Family| dir.join(format!("{run_tag}.{}.{n}.{}.tmp.parquet", std::process::id(), f.label()));
    let mut guards: Vec<TempFile> = Family::ALL.iter().map(|f| TempFile(Some(tmp(*f)))).collect();
    let mut script = format!("{prelude} CREATE TEMP TABLE r AS {select};");
    for f in Family::ALL {
        script.push_str(&format!(
            " COPY (SELECT * EXCLUDE ({DIM}){bbox} FROM r WHERE {DIM} = {} ORDER BY ST_Hilbert({g}, {{'min_x': -180, 'min_y': -90, 'max_x': 180, 'max_y': 90}}::BOX_2D)) TO {} (FORMAT parquet, ROW_GROUP_SIZE 16384);",
            dimension(f),
            literal(&tmp(f).display().to_string())
        ));
    }
    let counts =
        fetch(dir, &script, q.database, &format!("SELECT {DIM} AS dim, count(*)::BIGINT AS n FROM r GROUP BY ALL"), QUERY_TIMEOUT)?;

    let mut out = Output::default();
    for row in counts.as_array().into_iter().flatten() {
        let count = row.get("n").and_then(|v| v.as_u64()).unwrap_or(0);
        match row.get("dim").and_then(|v| v.as_i64()).and_then(|d| Family::ALL.into_iter().find(|f| dimension(*f) == d)) {
            Some(family) if count > 0 => {
                let path = dir.join(format!("{run_tag}.{}.parquet", family.label()));
                out.parts.push(Part { path, family, count });
            }
            Some(_) => {}
            None => out.skipped = out.skipped.saturating_add(count),
        }
    }
    out.parts.sort_by_key(|p| Family::ALL.iter().position(|f| *f == p.family));
    if out.parts.is_empty() {
        return Err(DataError::Format(if out.skipped > 0 {
            format!("none of the query's {} rows has a geometry", out.skipped)
        } else {
            NO_ROWS.into()
        }));
    }
    // The parts take their final names; the families not in the result are removed with their
    // guards.
    for p in &out.parts {
        let from = tmp(p.family);
        std::fs::rename(&from, &p.path)?;
        if let Some(g) = guards.iter_mut().find(|g| g.0.as_deref() == Some(from.as_path())) {
            g.0 = None;
        }
    }
    Ok(out)
}

/// The `ST_Dimension` of a geometry family.
fn dimension(f: Family) -> i64 {
    match f {
        Family::Points => 0,
        Family::Lines => 1,
        Family::Polygons => 2,
    }
}

/// The tables (and views) of a database that have a geometry column. Read confined: a view
/// reaching outside the database fails when it is converted.
pub fn tables(database: &Path) -> Result<Vec<Table>> {
    tables_of(&Database::File(database.to_path_buf()))
}

/// The tables (and views) of a database, file or connection, that have a geometry column.
pub fn tables_of(database: &Database) -> Result<Vec<Table>> {
    let dir = cache_dir().ok_or_else(|| DataError::Format("no cache folder for DuckDB results".into()))?;
    tables_in(&dir, database)
}

fn tables_in(dir: &Path, database: &Database) -> Result<Vec<Table>> {
    if !available() {
        return Err(missing());
    }
    std::fs::create_dir_all(dir)?;
    match database {
        Database::File(_) => {
            let select = format!(
                "SELECT DISTINCT schema_name, table_name FROM duckdb_columns() WHERE database_name = 'src' AND starts_with(data_type, 'GEOMETRY') ORDER BY ALL LIMIT {LIST_LIMIT}"
            );
            let rows = fetch(dir, &prelude(dir, Some(database), true)?, Some(database), &select, LIST_TIMEOUT)?;
            Ok(rows
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| {
                    let name = r.get("table_name")?.as_str()?.to_string();
                    if is_delta_or_internal_table(&name) {
                        return None;
                    }
                    Some(Table {
                        schema: r.get("schema_name")?.as_str()?.to_string(),
                        name,
                        geom_col: None,
                        geom_type: Some("GEOMETRY".into()),
                        feature_dataset: None,
                        srid: None,
                    })
                })
                .collect())
        }
        Database::Postgres(_) => postgres_tables_in(dir, database),
    }
}

/// Whether a table name is an internal Esri delta table (`a<id>`, `d<id>`), index table (`s<id>`,
/// `i<id>`, `f<id>`), or SDE/GDB bookkeeping table.
fn is_delta_or_internal_table(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("sde_") || lower.starts_with("gdb_") {
        return true;
    }
    if let Some(_first) = lower.chars().next().filter(|c| matches!(c, 'a' | 'd' | 'i' | 'f' | 's')) {
        let rest = &lower[1..];
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return true;
        }
        if rest.contains('_') && rest.chars().all(|c| c.is_ascii_digit() || c == '_') {
            return true;
        }
    }
    false
}

/// Introspects tables of a PostgreSQL database with a geometry column (PostGIS or Esri SDE).
fn postgres_tables_in(dir: &Path, database: &Database) -> Result<Vec<Table>> {
    let prelude = prelude(dir, Some(database), false)?;

    // Check if SDE / PostGIS catalog tables exist in PostgreSQL without failing
    let check_sde_sql = "SELECT \
        EXISTS (SELECT 1 FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'sde' AND c.relname = 'sde_geometry_columns') AS sde_geom, \
        EXISTS (SELECT 1 FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'sde' AND c.relname = 'sde_layers') AS sde_layers, \
        EXISTS (SELECT 1 FROM pg_catalog.pg_class c WHERE c.relname = 'gdb_items') AS gdb_items, \
        EXISTS (SELECT 1 FROM pg_catalog.pg_class c WHERE c.relname = 'geometry_columns') AS postgis_geom";

    let sde_check_query =
        format!("SELECT sde_geom, sde_layers, gdb_items, postgis_geom FROM postgres_query('src', {})", literal(check_sde_sql));

    let (has_sde_geom, has_sde_layers, has_gdb_items, has_postgis_geom) =
        match fetch(dir, &prelude, Some(database), &sde_check_query, LIST_TIMEOUT) {
            Ok(v) => {
                let row = v.as_array().and_then(|a| a.first());
                let g = row.and_then(|r| r.get("sde_geom")).and_then(|b| b.as_bool()).unwrap_or(false);
                let l = row.and_then(|r| r.get("sde_layers")).and_then(|b| b.as_bool()).unwrap_or(false);
                let gi = row.and_then(|r| r.get("gdb_items")).and_then(|b| b.as_bool()).unwrap_or(false);
                let pg = row.and_then(|r| r.get("postgis_geom")).and_then(|b| b.as_bool()).unwrap_or(false);
                (g, l, gi, pg)
            }
            Err(_) => (false, false, false, false),
        };

    let mut gdb_dataset_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if has_gdb_items {
        let gdb_rel_sql = "SELECT \
            COALESCE(fd.name, NULLIF(split_part(trim(both '\\/' from fc.path), '\\', 1), ''))::text AS feature_dataset, \
            fc.name::text AS feature_class, \
            fc.path::text AS item_path \
        FROM sde.gdb_items fc \
        LEFT JOIN sde.gdb_itemrelationships r ON (r.destid = fc.uuid OR r.originid = fc.uuid) \
        LEFT JOIN sde.gdb_items fd ON (fd.uuid = r.originid OR fd.uuid = r.destid) AND fd.uuid <> fc.uuid \
            AND ( \
                UPPER(REPLACE(REPLACE(fd.type::text, '{', ''), '}', '')) = '70737809-B030-489D-AE50-2A6A24762C7D' \
                OR fd.type::text ILIKE '%70737809-B030-489D-AE50-2A6A24762C7D%' \
            ) \
        WHERE ( \
            UPPER(REPLACE(REPLACE(fc.type::text, '{', ''), '}', '')) IN ( \
                'CD365147-3802-427E-AE63-8F4A7E4834A1', \
                'B719E022-77FA-48EB-84EE-47D87E4FA57A', \
                '70737809-B030-489D-AE50-2A6A24762C7D' \
            ) \
            OR fc.path LIKE '\\%\\%' \
            OR fd.name IS NOT NULL \
        );";
        let gdb_rel_query =
            format!("SELECT feature_dataset, feature_class, item_path FROM postgres_query('src', {})", literal(gdb_rel_sql));
        if let Ok(rows) = fetch(dir, &prelude, Some(database), &gdb_rel_query, LIST_TIMEOUT) {
            for r in rows.as_array().into_iter().flatten() {
                let fd_opt = r.get("feature_dataset").and_then(|v| v.as_str());
                let fc_opt = r.get("feature_class").and_then(|v| v.as_str());
                let path_opt = r.get("item_path").and_then(|v| v.as_str());
                if let (Some(fd), Some(fc)) = (fd_opt, fc_opt) {
                    let clean_fd = fd.rsplit('.').next().unwrap_or(fd).trim_matches('"').trim().to_string();
                    let clean_fc = fc.rsplit('.').next().unwrap_or(fc).trim_matches('"').trim().to_string();
                    if !clean_fd.is_empty() {
                        gdb_dataset_map.insert(fc.to_ascii_lowercase(), clean_fd.clone());
                        gdb_dataset_map.insert(clean_fc.to_ascii_lowercase(), clean_fd.clone());
                        if fc.contains('.') {
                            let parts: Vec<&str> = fc.split('.').collect();
                            if parts.len() >= 2 {
                                let suffix = format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1]);
                                gdb_dataset_map.insert(suffix.to_ascii_lowercase(), clean_fd.clone());
                            }
                        }
                    }
                }
                if let Some(p) = path_opt {
                    let segments: Vec<&str> = p.trim_matches('\\').split('\\').filter(|s| !s.is_empty()).collect();
                    if segments.len() >= 2 {
                        let ds = segments[0].trim_matches('"').trim().to_string();
                        let fc = segments[segments.len() - 1].trim_matches('"').trim().to_string();
                        if !ds.is_empty() && !fc.is_empty() {
                            gdb_dataset_map.entry(fc.to_ascii_lowercase()).or_insert(ds);
                        }
                    }
                }
            }
        }
    }

    let mut srid_map: std::collections::HashMap<(String, String), i32> = std::collections::HashMap::new();
    let mut srid_parts = Vec::new();
    if has_sde_layers {
        srid_parts.push(
            "SELECT l.owner::text AS table_schema, l.table_name::text AS table_name, COALESCE(NULLIF(sr.auth_srid, 0), l.srid)::integer AS srid \
             FROM sde.sde_layers l \
             LEFT JOIN sde.sde_spatial_references sr ON sr.srid = l.srid",
        );
    }
    if has_sde_geom {
        srid_parts.push(
            "SELECT g.f_table_schema::text AS table_schema, g.f_table_name::text AS table_name, COALESCE(NULLIF(sr.auth_srid, 0), g.srid)::integer AS srid \
             FROM sde.sde_geometry_columns g \
             LEFT JOIN sde.sde_spatial_references sr ON sr.srid = g.srid",
        );
    }
    if has_postgis_geom {
        srid_parts.push(
            "SELECT f_table_schema::text AS table_schema, f_table_name::text AS table_name, srid::integer AS srid \
             FROM geometry_columns WHERE srid > 0",
        );
    }
    if !srid_parts.is_empty() {
        let srid_sql = format!("SELECT * FROM ({}) AS u WHERE srid > 0", srid_parts.join(" UNION ALL "));
        let srid_select = format!("SELECT table_schema, table_name, srid FROM postgres_query('src', {})", literal(&srid_sql));
        if let Ok(rows) = fetch(dir, &prelude, Some(database), &srid_select, LIST_TIMEOUT) {
            for r in rows.as_array().into_iter().flatten() {
                if let (Some(s), Some(n), Some(srid)) = (
                    r.get("table_schema").and_then(|v| v.as_str()),
                    r.get("table_name").and_then(|v| v.as_str()),
                    r.get("srid").and_then(|v| v.as_i64()),
                ) {
                    srid_map.insert((s.to_ascii_lowercase(), n.to_ascii_lowercase()), srid as i32);
                    srid_map.insert(("".to_string(), n.to_ascii_lowercase()), srid as i32);
                }
            }
        }
    }

    let mut pg_scan_parts = Vec::new();

    // 1. Core catalog scan: pg_attribute & pg_type across all non-system schemas
    pg_scan_parts.push(
        "SELECT DISTINCT \
            n.nspname::text AS table_schema, \
            c.relname::text AS table_name, \
            a.attname::text AS column_name, \
            t.typname::text AS data_type, \
            tn.nspname::text AS type_schema \
        FROM pg_catalog.pg_class c \
        JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
        JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid \
        JOIN pg_catalog.pg_type t ON t.oid = a.atttypid \
        JOIN pg_catalog.pg_namespace tn ON tn.oid = t.typnamespace \
        WHERE c.relkind IN ('r', 'v', 'm', 'p', 'f') \
          AND a.attnum > 0 \
          AND NOT a.attisdropped \
          AND n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast') \
          AND c.relname !~ '^[adfi][0-9]+$' \
          AND c.relname !~ '^s[0-9]+$' \
          AND c.relname NOT LIKE 'sde_%' \
          AND c.relname NOT LIKE 'gdb_%' \
          AND ( \
              t.typname IN ('geometry', 'geography', 'st_geometry') \
              OR tn.nspname = 'sde' \
              OR t.typname ILIKE '%geometry%' \
              OR (n.nspname = 'sde' AND a.attname ILIKE 'shape%') \
          )",
    );

    // 2. SDE catalog tables if present
    if has_sde_geom {
        pg_scan_parts.push(
            "SELECT DISTINCT \
                n.nspname::text AS table_schema, \
                c.relname::text AS table_name, \
                g.f_geometry_column::text AS column_name, \
                'st_geometry'::text AS data_type, \
                'sde'::text AS type_schema \
            FROM sde.sde_geometry_columns g \
            JOIN pg_catalog.pg_class c ON lower(c.relname) = lower(g.f_table_name) \
            JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace AND lower(n.nspname) = lower(g.f_table_schema) \
            WHERE c.relkind IN ('r', 'v', 'm', 'p', 'f') \
              AND n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast') \
              AND c.relname !~ '^[adfi][0-9]+$' \
              AND c.relname !~ '^s[0-9]+$' \
              AND c.relname NOT LIKE 'sde_%' \
              AND c.relname NOT LIKE 'gdb_%'",
        );
    }

    if has_sde_layers {
        pg_scan_parts.push(
            "SELECT DISTINCT \
                n.nspname::text AS table_schema, \
                c.relname::text AS table_name, \
                l.spatial_column::text AS column_name, \
                'st_geometry'::text AS data_type, \
                'sde'::text AS type_schema \
            FROM sde.sde_layers l \
            JOIN pg_catalog.pg_class c ON lower(c.relname) = lower(l.table_name) \
            JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace AND lower(n.nspname) = lower(l.owner) \
            WHERE c.relkind IN ('r', 'v', 'm', 'p', 'f') \
              AND n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast') \
              AND c.relname !~ '^[adfi][0-9]+$' \
              AND c.relname !~ '^s[0-9]+$' \
              AND c.relname NOT LIKE 'sde_%' \
              AND c.relname NOT LIKE 'gdb_%'",
        );
    }

    let full_pg_sql = format!(
        "SELECT * FROM ({}) AS t WHERE table_name !~ '^[adfi][0-9]+$' AND table_name !~ '^s[0-9]+$' AND table_name NOT LIKE 'sde_%' AND table_name NOT LIKE 'gdb_%' ORDER BY table_schema, table_name LIMIT {LIST_LIMIT}",
        pg_scan_parts.join(" UNION ")
    );
    let duckdb_select = format!(
        "SELECT table_schema, table_name, column_name, data_type, type_schema FROM postgres_query('src', {})",
        literal(&full_pg_sql)
    );

    match fetch(dir, &prelude, Some(database), &duckdb_select, LIST_TIMEOUT) {
        Ok(rows) => {
            let mut out = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for r in rows.as_array().into_iter().flatten() {
                let schema = match r.get("table_schema").and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                let name = match r.get("table_name").and_then(|v| v.as_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                if is_delta_or_internal_table(&name) {
                    continue;
                }
                let col = r.get("column_name").and_then(|v| v.as_str()).map(str::to_string);
                let ty = r.get("data_type").and_then(|v| v.as_str()).unwrap_or("geometry").to_string();
                let type_schema = r.get("type_schema").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let full_ty = if !type_schema.is_empty() && type_schema != "public" { format!("{type_schema}.{ty}") } else { ty };

                let key = (schema.clone(), name.clone());
                if seen.insert(key) {
                    let dataset = gdb_dataset_map
                        .get(&name.to_ascii_lowercase())
                        .or_else(|| gdb_dataset_map.get(&format!("{schema}.{name}").to_ascii_lowercase()))
                        .cloned();
                    let table_srid = srid_map
                        .get(&(schema.to_ascii_lowercase(), name.to_ascii_lowercase()))
                        .or_else(|| srid_map.get(&("".to_string(), name.to_ascii_lowercase())))
                        .copied();
                    out.push(Table { schema, name, geom_col: col, geom_type: Some(full_ty), feature_dataset: dataset, srid: table_srid });
                }
            }
            Ok(out)
        }
        Err(e) => {
            // Fallback to duckdb_columns() if postgres_query encountered any unexpected issue
            let fallback_select = format!(
                "SELECT DISTINCT schema_name, table_name FROM duckdb_columns() WHERE database_name = 'src' AND starts_with(data_type, 'GEOMETRY') ORDER BY ALL LIMIT {LIST_LIMIT}"
            );
            let rows = fetch(dir, &prelude, Some(database), &fallback_select, LIST_TIMEOUT).map_err(|_| e)?;
            Ok(rows
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| {
                    let name = r.get("table_name")?.as_str()?.to_string();
                    if is_delta_or_internal_table(&name) {
                        return None;
                    }
                    Some(Table {
                        schema: r.get("schema_name")?.as_str()?.to_string(),
                        name,
                        geom_col: None,
                        geom_type: Some("GEOMETRY".into()),
                        feature_dataset: None,
                        srid: None,
                    })
                })
                .collect())
        }
    }
}

/// The available non-template databases of a PostgreSQL server.
pub fn databases_of(database: &Database) -> Result<Vec<String>> {
    let dir = cache_dir().ok_or_else(|| DataError::Format("no cache folder for DuckDB results".into()))?;
    if !available() {
        return Err(missing());
    }
    std::fs::create_dir_all(&dir)?;
    match database {
        Database::File(_) => Ok(Vec::new()),
        Database::Postgres(_) => {
            let prelude = prelude(&dir, Some(database), false)?;
            let pg_sql = "SELECT datname::text FROM pg_database WHERE datistemplate = false ORDER BY datname;";
            let select = format!("SELECT datname FROM postgres_query('src', {})", literal(pg_sql));
            let rows = fetch(&dir, &prelude, Some(database), &select, LIST_TIMEOUT)?;
            Ok(rows.as_array().into_iter().flatten().filter_map(|r| r.get("datname")?.as_str().map(str::to_string)).collect())
        }
    }
}

/// The available non-system schemas of a PostgreSQL database.
pub fn schemas_of(database: &Database) -> Result<Vec<String>> {
    let dir = cache_dir().ok_or_else(|| DataError::Format("no cache folder for DuckDB results".into()))?;
    if !available() {
        return Err(missing());
    }
    std::fs::create_dir_all(&dir)?;
    match database {
        Database::File(_) => Ok(vec!["main".to_string()]),
        Database::Postgres(_) => {
            let prelude = prelude(&dir, Some(database), false)?;
            let pg_sql = "SELECT nspname::text AS schema_name FROM pg_catalog.pg_namespace WHERE nspname NOT LIKE 'pg_%' AND nspname NOT IN ('information_schema', 'pg_toast') ORDER BY nspname;";
            let select = format!("SELECT schema_name FROM postgres_query('src', {})", literal(pg_sql));
            if let Ok(rows) = fetch(&dir, &prelude, Some(database), &select, LIST_TIMEOUT) {
                let list: Vec<String> =
                    rows.as_array().into_iter().flatten().filter_map(|r| r.get("schema_name")?.as_str().map(str::to_string)).collect();
                if !list.is_empty() {
                    return Ok(list);
                }
            }
            let duckdb_sql = "SELECT DISTINCT schema_name FROM duckdb_schemas() WHERE database_name = 'src' AND schema_name NOT LIKE 'pg_%' AND schema_name NOT IN ('information_schema') ORDER BY schema_name;";
            let rows = fetch(&dir, &prelude, Some(database), duckdb_sql, LIST_TIMEOUT)?;
            Ok(rows.as_array().into_iter().flatten().filter_map(|r| r.get("schema_name")?.as_str().map(str::to_string)).collect())
        }
    }
}

/// Tests a database connection.
pub fn test_connection(database: &Database) -> Result<()> {
    let dir = cache_dir().ok_or_else(|| DataError::Format("no cache folder for DuckDB results".into()))?;
    if !available() {
        return Err(missing());
    }
    std::fs::create_dir_all(&dir)?;
    let prelude = prelude(&dir, Some(database), false)?;
    match database {
        Database::File(_) => {
            let _ = fetch(&dir, &prelude, Some(database), "SELECT 1 AS ok", LIST_TIMEOUT)?;
            Ok(())
        }
        Database::Postgres(_) => {
            let select = format!("SELECT 1 AS ok FROM postgres_query('src', {})", literal("SELECT 1;"));
            let _ = fetch(&dir, &prelude, Some(database), &select, LIST_TIMEOUT)?;
            Ok(())
        }
    }
}

/// The table of a database named `name` (`table` or `schema.table`, any case).
pub fn find_table(database: &Path, name: &str) -> Result<Table> {
    find_table_of(&Database::File(database.to_path_buf()), name)
}

/// The table of a database, file or connection, named `name`.
pub fn find_table_of(database: &Database, name: &str) -> Result<Table> {
    let all = tables_of(database)?;
    let clean = name.trim().trim_matches('"');
    let dot_clean = if clean.contains('.') {
        clean.split('.').map(|part| part.trim_matches('"')).collect::<Vec<_>>().join(".")
    } else {
        clean.to_string()
    };
    all.into_iter()
        .find(|t| {
            t.label().eq_ignore_ascii_case(name)
                || t.label().eq_ignore_ascii_case(clean)
                || t.label().eq_ignore_ascii_case(&dot_clean)
                || t.name.eq_ignore_ascii_case(name)
                || t.name.eq_ignore_ascii_case(clean)
        })
        .ok_or_else(|| DataError::Format(format!("no table '{name}' with a geometry column")))
}

/// Converts a table of a database, reusing an earlier conversion while the file is unchanged.
pub fn convert_table(database: &Path, table: &Table, crs: Option<&str>) -> Result<Output> {
    convert_table_of(&Database::File(database.to_path_buf()), table, crs)
}

/// Converts a table of a database: a file's as [`convert_table`] does; a connection's afresh each
/// time (its data may change at any time).
pub fn convert_table_of(database: &Database, table: &Table, crs: Option<&str>) -> Result<Output> {
    let dir = cache_dir().ok_or_else(|| DataError::Format("no cache folder for DuckDB results".into()))?;
    let table_crs = table.crs();
    let effective_crs = crs.or(table_crs.as_deref());
    match database {
        Database::File(p) => convert_table_in(&dir, p, table, effective_crs),
        Database::Postgres(_) => {
            if !available() {
                return Err(missing());
            }
            std::fs::create_dir_all(&dir)?;
            let query_sql = table_query_sql(&dir, database, table)?;
            run_in(&dir, &Query { sql: &query_sql, database: Some(database), crs: effective_crs }, true)
        }
    }
}

fn table_query_sql(dir: &Path, database: &Database, table: &Table) -> Result<String> {
    if !table.is_sde() {
        return Ok(table.sql());
    }
    let col = table.geom_col.as_deref().unwrap_or("shape");
    let prelude = prelude(dir, Some(database), false)?;
    let col_sql = format!(
        "SELECT a.attname::text AS col \
         FROM pg_catalog.pg_attribute a \
         JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE lower(n.nspname) = lower({}) AND lower(c.relname) = lower({}) AND a.attnum > 0 AND NOT a.attisdropped \
         ORDER BY a.attnum;",
        literal(&table.schema),
        literal(&table.name)
    );
    let select = format!("SELECT col FROM postgres_query('src', {})", literal(&col_sql));
    let cols: Vec<String> = match fetch(dir, &prelude, Some(database), &select, LIST_TIMEOUT) {
        Ok(rows) => rows.as_array().into_iter().flatten().filter_map(|r| r.get("col")?.as_str().map(str::to_string)).collect(),
        Err(_) => Vec::new(),
    };

    let pg_sql = if !cols.is_empty() {
        let select_items: Vec<String> = cols
            .iter()
            .map(|c| if c.eq_ignore_ascii_case(col) { format!("sde.st_astext({}) AS geometry", ident(c)) } else { ident(c) })
            .collect();
        format!("SELECT {} FROM {}.{}", select_items.join(", "), ident(&table.schema), ident(&table.name))
    } else if col.eq_ignore_ascii_case("geometry") {
        format!("SELECT *, sde.st_astext({}) AS __rgis_wkt FROM {}.{}", ident(col), ident(&table.schema), ident(&table.name))
    } else {
        format!("SELECT *, sde.st_astext({}) AS geometry FROM {}.{}", ident(col), ident(&table.schema), ident(&table.name))
    };

    Ok(format!(
        "FROM (SELECT * EXCLUDE (geometry), ST_GeomFromText(geometry) AS geometry FROM postgres_query('src', {}))",
        literal(&pg_sql)
    ))
}

fn convert_table_in(dir: &Path, database: &Path, table: &Table, crs: Option<&str>) -> Result<Output> {
    let full = std::path::absolute(database).unwrap_or_else(|_| database.to_path_buf());
    let (len, modified) = stamp(&full)?;
    // Changes not yet checkpointed are in the write-ahead log beside the file.
    let mut wal = full.clone().into_os_string();
    wal.push(".wal");
    let (wal_len, wal_modified) = stamp(Path::new(&wal)).unwrap_or((0, 0));
    // The cache key, spelled out: kept beside the conversion and checked, so a hash collision
    // can't hand back another table.
    let key = format!("{}\n{}\n{}\n{crs:?}\n{len}\n{modified}\n{wal_len}\n{wal_modified}", full.display(), table.schema, table.name);
    let tag = format!("{:016x}", fnv1a(key.as_bytes()));
    let keyfile = dir.join(format!("{tag}.key"));
    if let Some(hit) = std::fs::read_to_string(&keyfile).ok().and_then(|s| cached(dir, &key, &s)) {
        touch(&keyfile);
        for p in &hit.parts {
            touch(&p.path);
        }
        return Ok(hit);
    }
    let _ = std::fs::remove_file(&keyfile);
    // A database is hostile input: its tables are read confined to the cache.
    let out = run_in(dir, &Query { sql: &table.sql(), database: Some(&Database::File(full.clone())), crs }, true)?;
    // The key goes last, vouching for the parts written before it: the key, then a line per part
    // (its family, rows and file) and the rows left out.
    let mut text = key.clone();
    for p in &out.parts {
        let file = p.path.file_name().and_then(|f| f.to_str()).unwrap_or_default();
        text.push_str(&format!("\npart {} {} {file}", p.family.label(), p.count));
    }
    text.push_str(&format!("\nskipped {}", out.skipped));
    std::fs::write(&keyfile, text)?;
    Ok(out)
}

/// A conversion in the cache, when its key file matches `key` and its parts are all there.
fn cached(dir: &Path, key: &str, stored: &str) -> Option<Output> {
    let rest = stored.strip_prefix(key)?.strip_prefix('\n')?;
    let mut out = Output::default();
    for line in rest.lines() {
        let mut w = line.split(' ');
        match (w.next(), w.next(), w.next(), w.next()) {
            // Only a file of this module's, in the cache folder.
            (Some("part"), Some(f), Some(n), Some(file)) if ours(file) && file.ends_with(&format!(".{f}.parquet")) => {
                let family = Family::ALL.into_iter().find(|x| x.label() == f)?;
                let path = dir.join(file);
                if !path.is_file() {
                    return None;
                }
                out.parts.push(Part { path, family, count: n.parse().ok()? });
            }
            (Some("skipped"), Some(n), None, None) => out.skipped = n.parse().ok()?,
            _ => return None,
        }
    }
    (!out.parts.is_empty()).then_some(out)
}

/// Whether a file name is one this module writes: `<16 hex>.<family>.parquet`, `<16 hex>.key`, or
/// a `<16 hex>.<pid>.<n>.<family>.tmp.parquet` or `<16 hex>.<pid>.<n>.rows.tmp.json` left by a
/// run that didn't finish.
fn ours(name: &str) -> bool {
    let Some((tag, rest)) = name.split_once('.') else { return false };
    if tag.len() != 16 || !tag.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    let family = |f: &str| Family::ALL.iter().any(|x| x.label() == f);
    if rest == "key" || rest.strip_suffix(".parquet").is_some_and(family) {
        return true;
    }
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let parts = |suffix: &str| rest.strip_suffix(suffix).map(|r| r.split('.').map(str::to_string).collect::<Vec<_>>());
    match (parts(".tmp.parquet").as_deref(), parts(".tmp.json").as_deref()) {
        (Some([pid, n, f]), _) => digits(pid) && digits(n) && family(f),
        (_, Some([pid, n, c])) => digits(pid) && digits(n) && c == "rows",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn databases_by_name() {
        // Quoted passwords with spaces, spaced `=`, escapes; an `@` past a URI's authority.
        assert_eq!(
            split_password("host=h  password = 'a b\\'c' application_name='my app' dbname=g"),
            ("host=h application_name='my app' dbname=g".to_string(), Some("a b'c".to_string()))
        );
        assert_eq!(split_password("host=h password='unterminated"), ("host=h".to_string(), None));
        assert_eq!(
            split_password("postgresql://db.example:5432/gis?application_name=x@y"),
            ("postgresql://db.example:5432/gis?application_name=x@y".to_string(), None)
        );
        assert_eq!(
            split_password("postgresql://me:p@ss@db.example/gis?x=1"),
            ("postgresql://me@db.example/gis?x=1".to_string(), Some("p@ss".to_string()))
        );
        assert_eq!(Database::parse(" places.duckdb "), Database::File("places.duckdb".into()));
        // A password is taken out of the connection string, remembered and never in its label.
        let d = Database::parse("postgres:host=db.example dbname=gis user=me password=pw1");
        assert_eq!(d, Database::Postgres("host=db.example dbname=gis user=me".into()));
        assert_eq!(d.label(), "postgres:host=db.example dbname=gis user=me");
        assert_eq!(d.short(), "gis");
        assert_eq!(password(Some(&d)).as_deref(), Some("pw1"));
        assert_eq!(Database::parse(&d.label()), d);
        let u = Database::parse("postgresql://me:pw2@db.example:5433/osm?sslmode=require");
        assert_eq!(u, Database::Postgres("postgresql://me@db.example:5433/osm?sslmode=require".into()));
        assert_eq!((u.label().as_str(), u.short().as_str()), ("postgresql://me@db.example:5433/osm?sslmode=require", "osm"));
        assert_eq!(password(Some(&u)).as_deref(), Some("pw2"));
        assert_eq!(Database::parse("POSTGRES:dbname=x"), Database::Postgres("dbname=x".into()));
        assert_eq!(password(Some(&Database::parse("postgres:dbname=other"))), None);
        // A connection is attached read only, its password never in the script.
        let dir = std::env::temp_dir();
        let p = prelude(&dir, Some(&d), true).unwrap();
        assert!(p.contains("ATTACH 'host=db.example dbname=gis user=me' AS src (TYPE postgres, READ_ONLY)") && !p.contains("pw1"), "{p}");
    }

    #[test]
    fn names_and_quoting() {
        assert!(is_duckdb_name("a.duckdb") && is_duckdb_name("B.DDB") && !is_duckdb_name("a.db") && !is_duckdb_name("duckdb"));
        assert_eq!(literal("it's"), "'it''s'");
        assert_eq!(ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(Table::new("main", "t").label(), "t");
        assert_eq!(Table::new("s", "t").label(), "s.t");
        let non_sde = Table::new("public", "parcels");
        assert!(!non_sde.is_sde());
        assert_eq!(non_sde.sql(), "FROM \"public\".\"parcels\"");
        let sde_table = Table {
            schema: "sde".into(),
            name: "boundaries".into(),
            geom_col: Some("shape".into()),
            geom_type: Some("sde.st_geometry".into()),
            feature_dataset: None,
            srid: None,
        };
        assert!(is_delta_or_internal_table("a1"));
        assert!(is_delta_or_internal_table("d402"));
        assert!(is_delta_or_internal_table("s12"));
        assert!(is_delta_or_internal_table("i15"));
        assert!(is_delta_or_internal_table("sde_layers"));
        assert!(is_delta_or_internal_table("gdb_items"));
        assert!(!is_delta_or_internal_table("lyr_distrb_box_evw"));
        assert!(!is_delta_or_internal_table("parcels"));
        assert!(sde_table.is_sde());
        assert!(sde_table.sql().contains("sde.st_astext(\"shape\") AS geometry"));
        assert!(sde_table.sql().contains("ST_GeomFromText(geometry) AS geometry"));
        assert!(sde_table.sql().contains("postgres_query('src'"));
        let wrapped = wrap_sde_query_if_needed("SELECT *, sde.st_astext(shape) AS geometry FROM \"sde\".\"roads\"");
        assert!(wrapped.contains("postgres_query('src'"));
        assert!(wrapped.contains("ST_GeomFromText(geometry) AS geometry"));
        assert!(check_crs(Some("EPSG:2263")).is_ok() && check_crs(None).is_ok());
        assert!(check_crs(Some("EPSG:2263'; --")).is_err() && check_crs(Some("")).is_err());
        assert_eq!(type_crs("GEOMETRY('EPSG:3857')"), Some("EPSG:3857"));
        assert_eq!(type_crs("GEOMETRY"), None);
        assert_eq!(strip_ansi("\u{1b}[90mWARNING\u{1b}[0m x"), "WARNING x");
        let m = message(
            "WARNING: x\nBinder Error: no column \"b\"\n\nLINE 1: SELECT … query('SELECT b')\n        ^",
            &std::process::ExitStatus::default(),
        );
        assert_eq!(m, "Binder Error: no column \"b\"");
    }

    #[test]
    fn cache_files() {
        assert!(ours("0123456789abcdef.points.parquet") && ours("0123456789abcdef.key"));
        assert!(ours("0123456789abcdef.4242.7.lines.tmp.parquet") && ours("0123456789abcdef.4242.7.rows.tmp.json"));
        assert!(!ours("0123456789abcdef.4242.7.lines.tmp.json") && !ours("0123456789abcdef.4242.7.rows.tmp.parquet"));
        assert!(!ours("0123456789abcdef.parquet") && !ours("0123456789abcdef.other.parquet") && !ours("init.sql"));
        assert!(!ours("data.points.parquet") && !ours("0123456789abcdef.x.7.lines.tmp.parquet"));
    }

    #[test]
    fn select_list() {
        let cols = |v: &[(&str, &str)]| v.iter().map(|(n, t)| Column { name: n.to_string(), ty: t.to_string() }).collect::<Vec<_>>();
        // No geometry column.
        let e = select(&cols(&[("a", "INTEGER")]), None, "x").unwrap_err();
        assert!(e.to_string().contains("no geometry column"), "{e}");
        // Types the provider doesn't read become text; a second geometry, WKT.
        let (s, g) =
            select(&cols(&[("d", "DATE"), ("geom", "GEOMETRY"), ("n", "DECIMAL(10,2)"), ("g2", "GEOMETRY('EPSG:3857')")]), None, "x")
                .unwrap();
        assert_eq!(g, "geom");
        assert!(s.contains("CAST(\"d\" AS VARCHAR)") && s.contains("ST_Force2D(\"geom\") AS \"geom\""), "{s}");
        assert!(s.contains(", \"n\",") && s.contains("ST_AsText(\"g2\")"), "{s}");
        // A geometry that says its coordinate system is converted; one given is used.
        let (s, _) = select(&cols(&[("g", "GEOMETRY('EPSG:3857')")]), None, "x").unwrap();
        assert!(s.contains("ST_Transform(\"g\", 'EPSG:4326')"), "{s}");
        let (s, _) = select(&cols(&[("g", "GEOMETRY('OGC:CRS84')")]), None, "x").unwrap();
        assert!(!s.contains("ST_Transform"), "{s}");
        let (s, _) = select(&cols(&[("g", "GEOMETRY")]), Some("EPSG:2263"), "x").unwrap();
        assert!(s.contains("ST_Transform(\"g\"::GEOMETRY, 'EPSG:2263', 'EPSG:4326')"), "{s}");
    }

    #[test]
    fn bad_queries_never_reach_duckdb() {
        let dir = std::env::temp_dir();
        for sql in ["", "  ;  ", "SELECT '\0'"] {
            assert!(run_in(&dir, &Query { sql, ..Query::default() }, false).is_err());
        }
        let long = "x".repeat(MAX_SQL + 1);
        assert!(run_in(&dir, &Query { sql: &long, ..Query::default() }, false).unwrap_err().to_string().contains("longer"));
        let e = run_in(&dir, &Query { sql: "SELECT 1", crs: Some("EPSG:1 OR"), ..Query::default() }, false).unwrap_err();
        assert!(e.to_string().contains("crs"), "{e}");
    }

    /// A query's first rows, as an SQL workspace shows them (skipped without DuckDB).
    #[test]
    fn previews_rows() {
        assert!(checked("  ;").is_err() && checked("SELECT 1\0").is_err());
        if !available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("rustgis-duckdb-preview-{}", std::process::id()));
        let sql = "SELECT i AS n, i AS n, 'x' || i AS label, DATE '2020-01-02' AS day, ST_Point(i, 2) AS geom FROM range(5) t(i)";
        let p = preview_in(&dir, &Query { sql, ..Query::default() }, 3).unwrap();
        let names: Vec<&str> = p.columns.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names.len(), 5, "{names:?}");
        assert!(p.geometry && p.more);
        assert_eq!(p.rows.len(), 3);
        assert_eq!(p.rows[1][2], serde_json::json!("x1"));
        assert_eq!(p.rows[1][3], serde_json::json!("2020-01-02"));
        assert_eq!(p.rows[1][4], serde_json::json!("POINT (1 2)"));
        // A table without geometry, all of it.
        let p = preview_in(&dir, &Query { sql: "SELECT 42 AS answer", ..Query::default() }, 10).unwrap();
        assert!(!p.geometry && !p.more);
        assert_eq!(p.rows, vec![vec![serde_json::json!(42)]]);
        // NaN and infinities show empty; big integers stay numbers.
        let sql = "SELECT 'NaN'::DOUBLE AS a, 'inf'::DOUBLE AS b, 1.5::DOUBLE AS c, 9007199254740993::BIGINT AS d, 7::UINTEGER AS e";
        let p = preview_in(&dir, &Query { sql, ..Query::default() }, 10).unwrap();
        assert_eq!(
            p.rows,
            vec![vec![
                serde_json::Value::Null,
                serde_json::Value::Null,
                serde_json::json!(1.5),
                serde_json::json!(9007199254740993i64),
                serde_json::json!(7)
            ]]
        );
        assert!(preview_in(&dir, &Query { sql: "SELECT * FROM nowhere", ..Query::default() }, 10).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Queries and a database through DuckDB when it is installed (skipped otherwise).
    #[test]
    fn runs_queries_when_duckdb_is_installed() {
        if !available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("rustgis-duckdb-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("cache");
        let open = |p: &Path| crate::source::open_path(p, None).unwrap();

        // Mixed geometry, a row without one, a projected column and types written as text.
        let sql = "SELECT 1 AS id, 'a' AS name, DATE '2020-01-02' AS day, ST_Point(10, 20) AS geom \
            UNION ALL SELECT 2, 'b', NULL, ST_GeomFromText('LINESTRING(0 0, 1 1)') \
            UNION ALL SELECT 3, 'c', NULL, ST_GeomFromText('POINT(11 21)') \
            UNION ALL SELECT 4, 'd', NULL, NULL;";
        let out = run_in(&cache, &Query { sql, ..Query::default() }, false).unwrap();
        assert_eq!(out.skipped, 1);
        let parts: Vec<(Family, u64)> = out.parts.iter().map(|p| (p.family, p.count)).collect();
        assert_eq!(parts, vec![(Family::Points, 2), (Family::Lines, 1)]);
        let pts = open(&out.parts[0].path);
        let info = pts.info();
        assert_eq!(info.count, Some(2));
        assert!(info.indexed, "the bbox covering is found");
        assert_eq!(info.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["id", "name", "day"]);
        assert_eq!(info.fields[2].ty, crate::FieldType::Text);
        let b = pts.read(&crate::source::Request { extent: Some(rustgis_geom::Extent::new(10.5, 20.5, 12.0, 22.0)), limit: 10 }).unwrap();
        assert_eq!(b.features.len(), 1, "the extent query reads the covering");
        // No temporary files are left.
        let names: Vec<String> =
            std::fs::read_dir(&cache).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().all(|n| !n.contains(".tmp.")), "{names:?}");

        // A projected geometry that says its coordinate system comes out in longitude/latitude.
        let out =
            run_in(&cache, &Query { sql: "SELECT ST_Point(1113194.9, 0)::GEOMETRY('EPSG:3857') AS g", ..Query::default() }, false).unwrap();
        let e = open(&out.parts[0].path).info().extent;
        assert!((e.min_x - 10.0).abs() < 1e-3 && e.min_y.abs() < 1e-6, "{e:?}");
        // …and so does one given with "crs".
        let q = Query { sql: "SELECT ST_Point(1113194.9, 0) AS g", crs: Some("EPSG:3857"), ..Query::default() };
        let e = open(&run_in(&cache, &q, false).unwrap().parts[0].path).info().extent;
        assert!((e.min_x - 10.0).abs() < 1e-3, "{e:?}");

        // Errors: DuckDB's own message, no geometry, not a single SELECT.
        let e = run_in(&cache, &Query { sql: "SELEC 1", ..Query::default() }, false).unwrap_err().to_string();
        assert!(e.contains("Parser Error"), "{e}");
        let e =
            run_in(&cache, &Query { sql: "SELECT ST_Point(0, 0) AS g WHERE false", ..Query::default() }, false).unwrap_err().to_string();
        assert_eq!(e, "the query returned no rows");
        let e = run_in(&cache, &Query { sql: "SELECT 1 AS a", ..Query::default() }, false).unwrap_err().to_string();
        assert!(e.contains("no geometry column"), "{e}");
        for sql in ["COPY (SELECT 1) TO 'x.csv'", "SELECT ST_Point(0, 0) AS g'); COPY (SELECT 1) TO ('x.csv", "SELECT 1; SELECT 2"] {
            assert!(run_in(&cache, &Query { sql, ..Query::default() }, false).is_err(), "{sql}");
        }
        assert!(!dir.join("x.csv").exists() && !Path::new("x.csv").exists());

        // A database: its geometry tables, converted once.
        let db = dir.join("t.duckdb");
        let secret = dir.join("secret.txt");
        std::fs::write(&secret, "do not read").unwrap();
        let duckdb = program().unwrap();
        let st = Command::new(duckdb)
            .arg(&db)
            .arg("-c")
            .arg("LOAD spatial; CREATE SCHEMA s2; CREATE TABLE pts AS SELECT i AS id, ST_Point(i % 360 - 180, i % 170 - 85) AS geom FROM range(1000) t(i); CREATE TABLE s2.polys AS SELECT 1 AS id, ST_GeomFromText('POLYGON((0 0, 1 0, 1 1, 0 0))') AS geom; CREATE TABLE plain(a INT);")
            .arg("-c")
            .arg(format!("CREATE VIEW leak AS SELECT content AS c, ST_Point(0, 0) AS geom FROM read_text({});", literal(&secret.display().to_string())))
            .status()
            .unwrap();
        assert!(st.success());
        let tables = tables_in(&cache, &Database::File(db.clone())).unwrap();
        assert_eq!(tables.iter().map(Table::label).collect::<Vec<_>>(), vec!["leak", "pts", "s2.polys"]);
        // A view reaching outside the database is refused: tables are read confined to the cache.
        let e = convert_table_in(&cache, &db, &tables[0], None).unwrap_err().to_string();
        assert!(e.contains("Permission Error"), "{e}");
        let tables = &tables[1..];
        let a = convert_table_in(&cache, &db, &tables[0], None).unwrap();
        assert_eq!(a.parts.iter().map(|p| p.count).collect::<Vec<_>>(), vec![1000]);
        // The second time is a cache hit, read back from the key file.
        assert_eq!(convert_table_in(&cache, &db, &tables[0], None).unwrap(), a);
        // Changes still in the write-ahead log are a different file.
        let mut wal = db.clone().into_os_string();
        wal.push(".wal");
        std::fs::write(&wal, b"pending").unwrap();
        let fresh = convert_table_in(&cache, &db, &tables[0], None).unwrap();
        assert_ne!(fresh.parts[0].path, a.parts[0].path);
        std::fs::remove_file(&wal).unwrap();
        let key = std::fs::read_dir(&cache).unwrap().flatten().find(|e| e.file_name().to_string_lossy().ends_with(".key")).unwrap();
        assert!(std::fs::read_to_string(key.path()).unwrap().contains("part points 1000 "));
        // A query naming the database's tables plainly.
        let q = Query { sql: "SELECT * FROM pts WHERE id < 10", database: Some(&Database::File(db.clone())), crs: None };
        assert_eq!(run_in(&cache, &q, false).unwrap().parts[0].count, 10);
        let _ = std::fs::remove_dir_all(dir);
    }
}
