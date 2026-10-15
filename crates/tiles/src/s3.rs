//! Amazon S3 and S3-compatible storage (MinIO, Cloudflare R2, Wasabi, source.coop): `s3://`
//! addresses, connections with their credentials, AWS Signature Version 4, and bucket listings.
//!
//! An `s3://bucket/key` address is fetched from the endpoint of the connection that serves its
//! bucket (AWS when none does), and requests to a connection's own endpoint are signed with its
//! credentials. Credentials stay in memory for the session: they are never in a project, a
//! command's result, a notice or the log, and they are only ever sent to the endpoint they are
//! for (a signed request that is redirected is not followed). The fetchers ([`crate::cache`] and
//! the web app's) call [`prepare`] for every web request.

use std::sync::Mutex;

use sha2::{Digest, Sha256};

/// The region used when none is given.
pub const DEFAULT_REGION: &str = "us-east-1";
/// How many connections a session holds.
const MAX_CONNECTIONS: usize = 32;
/// The payload hash of a request whose body isn't signed (S3 GETs).
const UNSIGNED: &str = "UNSIGNED-PAYLOAD";

/// An access key and its secret (and a session token for temporary credentials).
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

// Never printed: a log line or an error built with `{:?}` shows no secret.
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials(…)")
    }
}

/// A named way to reach buckets: an endpoint (AWS when `None`), a region, the buckets it serves
/// (every bucket when empty) and its credentials (anonymous when `None`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    pub name: String,
    /// `https://host[:port]` (or `http://` for a server on this computer or network), no path.
    pub endpoint: Option<String>,
    pub region: String,
    pub buckets: Vec<String>,
    pub credentials: Option<Credentials>,
}

impl Connection {
    /// What a command may show of it: never the credentials.
    pub fn public(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "endpoint": self.endpoint,
            "region": self.region,
            "buckets": self.buckets,
            "signed": self.credentials.is_some(),
        })
    }
}

fn store() -> &'static Mutex<Vec<Connection>> {
    static S: std::sync::OnceLock<Mutex<Vec<Connection>>> = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

fn lock() -> std::sync::MutexGuard<'static, Vec<Connection>> {
    store().lock().unwrap_or_else(|e| e.into_inner())
}

/// Adds (or replaces, by name) a connection for the session, after checking it.
pub fn connect(mut c: Connection) -> Result<(), String> {
    c.name = c.name.trim().to_string();
    if c.name.is_empty() || c.name.chars().count() > 64 || c.name.chars().any(char::is_control) {
        return Err("a connection needs a name of up to 64 characters".into());
    }
    c.endpoint = match c.endpoint.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
        Some(e) => Some(endpoint(e)?),
        None => None,
    };
    c.region = c.region.trim().to_string();
    if c.region.is_empty() {
        c.region = DEFAULT_REGION.into();
    }
    if c.region.len() > 32 || !c.region.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(format!("'{}' isn't a region name", c.region));
    }
    c.buckets.retain(|b| !b.trim().is_empty());
    for b in &mut c.buckets {
        *b = b.trim().to_string();
        if !valid_bucket(b) {
            return Err(format!("'{b}' isn't a bucket name"));
        }
    }
    c.buckets.truncate(64);
    if let Some(cr) = &c.credentials {
        let ok = |s: &str, max: usize| !s.is_empty() && s.len() <= max && !s.chars().any(|ch| ch.is_control() || ch.is_whitespace());
        if !ok(&cr.access_key_id, 128) || !ok(&cr.secret_access_key, 256) || cr.session_token.as_deref().is_some_and(|t| !ok(t, 4096)) {
            return Err("the access key, secret or session token isn't well formed".into());
        }
    }
    let mut all = lock();
    all.retain(|o| o.name != c.name);
    if all.len() >= MAX_CONNECTIONS {
        return Err(format!("up to {MAX_CONNECTIONS} connections"));
    }
    // A connection replaced by name starts afresh: the buckets granted to the old one (browsed
    // through another endpoint or with other keys) aren't the new one's.
    granted().retain(|(n, _)| n != &c.name);
    all.push(c);
    Ok(())
}

/// Forgets a connection (and its credentials, and the buckets granted to it); whether there was
/// one by that name.
pub fn disconnect(name: &str) -> bool {
    let mut all = lock();
    let before = all.len();
    all.retain(|c| c.name != name);
    granted().retain(|(n, _)| n != name);
    all.len() != before
}

fn granted() -> std::sync::MutexGuard<'static, Vec<(String, String)>> {
    static G: std::sync::OnceLock<Mutex<Vec<(String, String)>>> = std::sync::OnceLock::new();
    G.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Lets a connection serve a bucket for the rest of the session: the user browsed or added it
/// through that connection.
pub fn grant(name: &str, bucket: &str) {
    if !valid_bucket(bucket) || connection(name).is_none() {
        return;
    }
    let mut g = granted();
    g.retain(|(_, b)| b != bucket);
    if g.len() < 4096 {
        g.push((name.to_string(), bucket.to_string()));
    }
}

/// Whether a connection serves a bucket: it was given the bucket, or the bucket was granted to it.
fn serves(c: &Connection, bucket: &str) -> bool {
    c.buckets.iter().any(|b| b == bucket) || granted().iter().any(|(n, b)| *n == c.name && b == bucket)
}

/// The session's connections.
pub fn connections() -> Vec<Connection> {
    lock().clone()
}

/// A connection by name.
pub fn connection(name: &str) -> Option<Connection> {
    lock().iter().find(|c| c.name == name).cloned()
}

/// An endpoint as connections keep it: a web address's scheme and authority, no path.
fn endpoint(e: &str) -> Result<String, String> {
    let e = e.trim().trim_end_matches('/');
    let lower = e.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err(format!("{e}: an endpoint is an https:// (or http://) address"));
    }
    let (scheme, rest) = e.split_once("://").unwrap_or(("https", e));
    if rest.is_empty() || rest.contains(['/', '?', '#', '@']) {
        return Err(format!("{e}: an endpoint is a scheme and a host (and port), such as https://s3.example.org"));
    }
    let e = origin(scheme, rest);
    crate::address::check(&e, None)?;
    Ok(e)
}

/// A scheme and authority as compared: lower-case, without the scheme's default port.
fn origin(scheme: &str, authority: &str) -> String {
    let (scheme, authority) = (scheme.to_ascii_lowercase(), authority.to_ascii_lowercase());
    let default = if scheme == "https" { ":443" } else { ":80" };
    let authority = authority.strip_suffix(default).unwrap_or(&authority);
    format!("{scheme}://{authority}")
}

/// Whether `b` is a bucket name S3 accepts (3–63 lower-case letters, digits, dots and hyphens).
pub fn valid_bucket(b: &str) -> bool {
    let bytes = b.as_bytes();
    (3..=63).contains(&bytes.len())
        && bytes.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'.' || *c == b'-')
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
}

/// Whether an address is an `s3://` one.
pub fn is_s3(url: &str) -> bool {
    url.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("s3://"))
}

/// The bucket and key of an `s3://bucket/key` address.
pub fn parse(url: &str) -> Result<(String, String), String> {
    let rest = url.get(5..).filter(|_| is_s3(url)).ok_or_else(|| format!("{url}: not an s3:// address"))?;
    let (bucket, key) = rest.split_once('/').unwrap_or((rest, ""));
    if !valid_bucket(bucket) {
        return Err(format!("{url}: '{bucket}' isn't a bucket name"));
    }
    if key.len() > 1024 || key.chars().any(char::is_control) {
        return Err(format!("{url}: the key isn't one S3 takes"));
    }
    Ok((bucket.to_string(), key.to_string()))
}

/// The connection serving a bucket: one given the bucket, or one it was granted to this session
/// ([`grant`]). None serves any other bucket: an address naming it (a project's, a link's) is read
/// anonymously from AWS, never with the user's keys or from the user's endpoint.
pub fn for_bucket(bucket: &str) -> Option<Connection> {
    let all = lock();
    all.iter().find(|c| serves(c, bucket)).cloned()
}

/// Starts an address read anonymously whatever the connections: nothing signed, and an `s3://`
/// address read from AWS (the S3 dialog's "AWS, anonymous").
pub const ANONYMOUS: &str = "rustgis-anonymous:";

/// An address read anonymously ([`ANONYMOUS`]).
pub fn anonymous(url: &str) -> String {
    format!("{ANONYMOUS}{url}")
}

/// Splits off [`ANONYMOUS`]: the address, and whether it is read anonymously.
pub fn split_anonymous(url: &str) -> (&str, bool) {
    match url.strip_prefix(ANONYMOUS) {
        Some(u) => (u, true),
        None => (url, false),
    }
}

/// The web address of an object (or, with an empty key, of its bucket) through a connection:
/// path-style at a connection's own endpoint; virtual-hosted at AWS (path-style for a bucket
/// with dots, which doesn't fit a certificate). Without a connection, AWS's global endpoint,
/// which reaches a bucket in any region.
pub fn object_url(bucket: &str, key: &str, conn: Option<&Connection>) -> String {
    let key = encode(key, false);
    match (conn.and_then(|c| c.endpoint.as_deref()), conn) {
        (Some(ep), _) => format!("{ep}/{bucket}/{key}"),
        (None, Some(c)) if bucket.contains('.') => format!("https://s3.{}.amazonaws.com/{bucket}/{key}", c.region),
        (None, Some(c)) => format!("https://{bucket}.s3.{}.amazonaws.com/{key}", c.region),
        (None, None) if bucket.contains('.') => format!("https://s3.amazonaws.com/{bucket}/{key}"),
        (None, None) => format!("https://{bucket}.s3.amazonaws.com/{key}"),
    }
}

/// The web address an `s3://` address is fetched from.
pub fn https_url(url: &str) -> Result<String, String> {
    let (url, anonymous) = split_anonymous(url);
    let (bucket, key) = parse(url)?;
    let conn = if anonymous { None } else { for_bucket(&bucket) };
    Ok(object_url(&bucket, &key, conn.as_ref()))
}

/// A request ready to send: its web address, the headers to add, and whether it is signed (a
/// signed request is never redirected elsewhere).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub signed: bool,
}

/// Makes a GET of `url` ready: an `s3://` address becomes its web address, and a request to a
/// connection's endpoint (or to AWS S3 for an AWS connection) is signed with its credentials.
/// Other addresses pass unchanged.
pub fn prepare(url: &str) -> Result<Prepared, String> {
    let (url, anonymous) = split_anonymous(url);
    let (url, signer) = if is_s3(url) {
        let (bucket, key) = parse(url)?;
        let conn = if anonymous { None } else { for_bucket(&bucket) };
        let region = conn.as_ref().map(|c| c.region.clone()).unwrap_or_default();
        (object_url(&bucket, &key, conn.as_ref()), conn.map(|c| (c, region)))
    } else {
        (url.to_string(), if anonymous { None } else { signer_for(url) })
    };
    let Some((c, region)) = signer else {
        return Ok(Prepared { url, headers: Vec::new(), signed: false });
    };
    let Some(cr) = c.credentials.as_ref() else {
        return Ok(Prepared { url, headers: Vec::new(), signed: false });
    };
    // Sent in the form it is signed in: the path's parts encoded as S3 encodes them.
    let url = canonical_url(&url)?;
    let headers = sign_get(&url, &region, cr, &amz_date(now_secs()))?;
    Ok(Prepared { url, headers, signed: true })
}

/// A web address with its path in S3's canonical form: each part decoded, then encoded.
fn canonical_url(url: &str) -> Result<String, String> {
    let (scheme, rest) = url.split_once("://").ok_or_else(|| format!("{url}: not a web address"))?;
    let rest = rest.split('#').next().unwrap_or(rest);
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, path_query) = rest.split_at(end);
    let (path, query) = match path_query.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path_query, None),
    };
    let mut out = format!("{scheme}://{authority}{}", canonical_path(path));
    if let Some(q) = query {
        out.push('?');
        out.push_str(q);
    }
    Ok(out)
}

/// A path as S3 signs it: each part decoded (a `+` is a plus) and encoded again.
fn canonical_path(path: &str) -> String {
    if path.is_empty() {
        return "/".into();
    }
    path.split('/').map(|part| encode(&decode_path(part), true)).collect::<Vec<_>>().join("/")
}

/// The connection whose credentials sign a request to `url`, and the region it is signed for:
/// one whose endpoint is the address's scheme and authority (default ports aside), or (an AWS
/// connection) an address at AWS S3 of a bucket the connection serves ([`for_bucket`]): never
/// another bucket at amazonaws.com.
fn signer_for(url: &str) -> Option<(Connection, String)> {
    let lower = url.to_ascii_lowercase();
    let (scheme, rest) = lower.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') {
        return None;
    }
    let here = origin(scheme, authority);
    let all = lock();
    if let Some(c) = all.iter().find(|c| c.credentials.is_some() && c.endpoint.as_deref() == Some(here.as_str())) {
        return Some((c.clone(), c.region.clone()));
    }
    let host = authority.split(':').next().unwrap_or(authority);
    if scheme != "https"
        || here != format!("https://{host}")
        || !host.ends_with(".amazonaws.com")
        || !(host.starts_with("s3.") || host.contains(".s3.") || host.starts_with("s3-") || host.contains(".s3-"))
    {
        return None;
    }
    let c = match aws_bucket(host, rest) {
        Some(bucket) => all.iter().find(|c| c.credentials.is_some() && c.endpoint.is_none() && serves(c, &bucket))?,
        // The service itself (`https://s3.<region>.amazonaws.com/`, no bucket): the account's
        // bucket list (`buckets_url`), signed by an AWS connection of that region. It names no
        // bucket, so no one else's server sees the request.
        None => {
            let path = rest.split_once('/').map(|(_, p)| p).unwrap_or("");
            let region = host.strip_prefix("s3.").and_then(|r| r.strip_suffix(".amazonaws.com"))?;
            if !path.split(['?', '#']).next().unwrap_or("").is_empty() {
                return None;
            }
            all.iter().find(|c| c.credentials.is_some() && c.endpoint.is_none() && c.region == region)?
        }
    };
    // The region the address names (s3.<region>.amazonaws.com), else the connection's.
    let after = host
        .rsplit_once(".s3.")
        .map(|(_, a)| a)
        .or_else(|| host.strip_prefix("s3."))
        .or_else(|| host.rsplit_once(".s3-").map(|(_, a)| a))
        .or_else(|| host.strip_prefix("s3-"));
    let region = after
        .and_then(|a| a.split('.').next())
        .filter(|r| !r.is_empty() && *r != "amazonaws" && *r != "dualstack")
        .map(str::to_string)
        .unwrap_or_else(|| c.region.clone());
    Some((c.clone(), region))
}

/// The bucket an AWS S3 address names: its host's first label (virtual-hosted) or its path's
/// first part (path-style at `s3.<region>.amazonaws.com`).
fn aws_bucket(host: &str, rest: &str) -> Option<String> {
    // Virtual-hosted: bucket.s3.amazonaws.com, bucket.s3.<region>.amazonaws.com, or the legacy
    // bucket.s3-<region>.amazonaws.com.
    if let Some(b) = host
        .split_once(".s3.")
        .map(|(b, _)| b)
        .or_else(|| host.strip_suffix(".s3.amazonaws.com"))
        .or_else(|| host.split_once(".s3-").map(|(b, _)| b))
    {
        return Some(b.to_string());
    }
    let path = rest.split_once('/').map(|(_, p)| p).unwrap_or("");
    path.split(['/', '?', '#']).next().filter(|b| valid_bucket(b)).map(str::to_string)
}

/// The headers that sign a GET of `url` for S3 at `date` (`YYYYMMDDTHHMMSSZ`).
fn sign_get(url: &str, region: &str, cr: &Credentials, date: &str) -> Result<Vec<(String, String)>, String> {
    let (host, path, query) = split_url(url)?;
    let mut headers = vec![
        ("host".to_string(), host.clone()),
        ("x-amz-content-sha256".to_string(), UNSIGNED.to_string()),
        ("x-amz-date".to_string(), date.to_string()),
    ];
    if let Some(t) = &cr.session_token {
        headers.push(("x-amz-security-token".to_string(), t.clone()));
    }
    let auth = authorization(
        &Request { method: "GET", path: &path, query: &query, headers: &headers, payload_hash: UNSIGNED },
        region,
        "s3",
        date,
        cr,
    );
    // The browser and the HTTP client set Host themselves.
    let mut out: Vec<(String, String)> = headers.into_iter().filter(|(k, _)| k != "host").collect();
    out.push(("authorization".to_string(), auth));
    Ok(out)
}

/// A web address's host (with its port), path and query.
fn split_url(url: &str) -> Result<(String, String, String), String> {
    let rest = url.split_once("://").map(|(_, r)| r).ok_or_else(|| format!("{url}: not a web address"))?;
    let rest = rest.split('#').next().unwrap_or(rest);
    let (authority, path_query) = match rest.find('/') {
        Some(i) => (rest.get(..i).unwrap_or(""), rest.get(i..).unwrap_or("/")),
        None => match rest.find('?') {
            Some(i) => (rest.get(..i).unwrap_or(""), rest.get(i..).unwrap_or("")),
            None => (rest, "/"),
        },
    };
    let (path, query) = path_query.split_once('?').unwrap_or((path_query, ""));
    let path = if path.is_empty() { "/" } else { path };
    // The default port isn't part of Host.
    let scheme = url.split_once("://").map(|(s, _)| s).unwrap_or("https");
    let host = origin(scheme, authority).split_once("://").map(|(_, h)| h.to_string()).unwrap_or_default();
    Ok((host, path.to_string(), query.to_string()))
}

/// A request as Signature Version 4 sees it. `headers` are lower-case names (Host included).
pub struct Request<'a> {
    pub method: &'a str,
    /// The path as sent (already encoded).
    pub path: &'a str,
    /// The query as sent, or empty.
    pub query: &'a str,
    pub headers: &'a [(String, String)],
    pub payload_hash: &'a str,
}

/// The Authorization header of a request (Signature Version 4).
pub fn authorization(r: &Request<'_>, region: &str, service: &str, date: &str, cr: &Credentials) -> String {
    let day = date.get(..8).unwrap_or(date);
    let mut headers: Vec<(String, String)> =
        r.headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.split_whitespace().collect::<Vec<_>>().join(" "))).collect();
    headers.sort();
    let signed: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
    let signed = signed.join(";");
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let canonical = format!("{}\n{}\n{}\n{canonical_headers}\n{signed}\n{}", r.method, r.path, canonical_query(r.query), r.payload_hash);
    let scope = format!("{day}/{region}/{service}/aws4_request");
    let to_sign = format!("AWS4-HMAC-SHA256\n{date}\n{scope}\n{}", hex(&Sha256::digest(canonical.as_bytes())));
    let key = [day, region, service, "aws4_request"]
        .iter()
        .fold(format!("AWS4{}", cr.secret_access_key).into_bytes(), |k, part| hmac(&k, part.as_bytes()).to_vec());
    let signature = hex(&hmac(&key, to_sign.as_bytes()));
    format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}", cr.access_key_id)
}

/// A query in canonical form: each name and value encoded, sorted.
fn canonical_query(q: &str) -> String {
    let mut pairs: Vec<(String, String)> = q
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (encode(&decode(k), true), encode(&decode(v), true))
        })
        .collect();
    pairs.sort();
    pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

/// Percent-encodes as S3 wants: everything but unreserved characters (and `/` unless asked).
pub fn encode(s: &str, slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b'/' if !slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Decodes a path's percent-escapes (a `+` stays a plus).
fn decode_path(s: &str) -> String {
    decode(&s.replace('+', "%2B"))
}

/// Decodes percent-escapes (and `+` as a space, as query strings do).
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hexval = |c: u8| (c as char).to_digit(16);
        match (b, bytes.get(i + 1).and_then(|&c| hexval(c)), bytes.get(i + 2).and_then(|&c| hexval(c))) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (b'+', _, _) => {
                out.push(b' ');
                i += 1;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// HMAC-SHA256.
fn hmac(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |x: u8| k.iter().map(|b| b ^ x).collect::<Vec<u8>>();
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(msg).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().into()
}

fn now_secs() -> i64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `YYYYMMDDTHHMMSSZ` of a Unix time.
fn amz_date(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// How many entries a listing returns at most.
pub const MAX_LIST: usize = 10_000;

/// The request listing a bucket's objects under a prefix, a level at a time (ListObjectsV2 with
/// `/` as the delimiter), from `token` when continuing.
pub fn list_url(bucket: &str, prefix: &str, token: Option<&str>, conn: Option<&Connection>) -> String {
    let base = object_url(bucket, "", conn);
    // `https://bucket.s3…/` or `endpoint/bucket/`: the bucket itself, without the slash.
    let base = if conn.and_then(|c| c.endpoint.as_deref()).is_some() || bucket.contains('.') {
        base.trim_end_matches('/').to_string()
    } else {
        base
    };
    let mut q = format!("{base}?list-type=2&delimiter=%2F&max-keys=1000&prefix={}", encode(prefix, true));
    if let Some(t) = token {
        q.push_str(&format!("&continuation-token={}", encode(t, true)));
    }
    q
}

/// The request listing a connection's buckets (ListBuckets; credentials needed).
pub fn buckets_url(conn: &Connection) -> String {
    match &conn.endpoint {
        Some(ep) => format!("{ep}/"),
        None => format!("https://s3.{}.amazonaws.com/", conn.region),
    }
}

/// An object in a listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Object {
    pub key: String,
    pub size: u64,
    pub modified: String,
}

/// A level of a bucket: its "folders" (prefixes ending in `/`), its objects, and where the next
/// page starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub prefixes: Vec<String>,
    pub objects: Vec<Object>,
    pub next: Option<String>,
}

/// An S3 error answer's code and message, if `xml` is one.
fn error_of(doc: &roxmltree::Document) -> Option<String> {
    let root = doc.root_element();
    (root.tag_name().name() == "Error").then(|| {
        let text = |n: &str| root.children().find(|c| c.tag_name().name() == n).and_then(|c| c.text()).unwrap_or("").to_string();
        format!("{}: {}", text("Code"), text("Message")).chars().take(300).collect()
    })
}

fn parse_xml(bytes: &[u8]) -> Result<roxmltree::Document<'_>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the answer isn't UTF-8 text".to_string())?;
    // Through the data crate's depth check: a listing is hostile input like any document.
    rustgis_data::xml::parse(text, roxmltree::ParsingOptions::default()).map_err(|_| "the answer isn't an S3 listing".to_string())
}

/// A ListObjectsV2 answer.
pub fn parse_list(bytes: &[u8]) -> Result<Listing, String> {
    let doc = parse_xml(bytes)?;
    if let Some(e) = error_of(&doc) {
        return Err(e);
    }
    let root = doc.root_element();
    if root.tag_name().name() != "ListBucketResult" {
        return Err("the answer isn't an S3 listing".into());
    }
    let child = |n: &roxmltree::Node, name: &str| {
        n.children().find(|c| c.tag_name().name() == name).and_then(|c| c.text()).unwrap_or("").to_string()
    };
    let mut out = Listing::default();
    for n in root.children().filter(|c| c.is_element()).take(MAX_LIST) {
        match n.tag_name().name() {
            "Contents" => {
                let key = child(&n, "Key");
                if !key.is_empty() && key.len() <= 1024 {
                    out.objects.push(Object { key, size: child(&n, "Size").parse().unwrap_or(0), modified: child(&n, "LastModified") });
                }
            }
            "CommonPrefixes" => {
                let p = child(&n, "Prefix");
                if !p.is_empty() && p.len() <= 1024 {
                    out.prefixes.push(p);
                }
            }
            _ => {}
        }
    }
    let truncated = child(&root, "IsTruncated") == "true";
    out.next = Some(child(&root, "NextContinuationToken")).filter(|t| truncated && !t.is_empty() && t.len() <= 4096);
    Ok(out)
}

/// A ListBuckets answer: the bucket names.
pub fn parse_buckets(bytes: &[u8]) -> Result<Vec<String>, String> {
    let doc = parse_xml(bytes)?;
    if let Some(e) = error_of(&doc) {
        return Err(e);
    }
    Ok(doc
        .descendants()
        .filter(|n| n.tag_name().name() == "Bucket")
        .filter_map(|b| b.children().find(|c| c.tag_name().name() == "Name").and_then(|c| c.text()))
        .filter(|b| valid_bucket(b))
        .take(1000)
        .map(str::to_string)
        .collect())
}

/// What an AWS profile gives, from the text of `~/.aws/credentials` and `~/.aws/config`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub credentials: Option<Credentials>,
    pub region: Option<String>,
    pub endpoint: Option<String>,
}

/// Reads a profile from the shared credentials and config files' text: `[name]` in the
/// credentials file, `[profile name]` (or `[default]`) in the config file.
pub fn profile(credentials_ini: &str, config_ini: &str, name: &str) -> Profile {
    fn section<'a>(ini: &'a str, header: &str) -> Vec<(&'a str, &'a str)> {
        let mut inside = false;
        let mut out = Vec::new();
        for line in ini.lines().take(10_000) {
            let line = line.trim();
            if line.starts_with('[') {
                inside = line.trim_start_matches('[').trim_end_matches(']').trim() == header;
            } else if inside
                && !line.starts_with(['#', ';'])
                && let Some((k, v)) = line.split_once('=')
            {
                out.push((k.trim(), v.trim()));
            }
        }
        out
    }
    let creds = section(credentials_ini, name);
    let config_header = if name == "default" { "default".to_string() } else { format!("profile {name}") };
    let config = section(config_ini, &config_header);
    let get = |k: &str| {
        creds.iter().chain(config.iter()).find(|(key, _)| key.eq_ignore_ascii_case(k)).map(|(_, v)| v.to_string()).filter(|v| !v.is_empty())
    };
    let credentials = match (get("aws_access_key_id"), get("aws_secret_access_key")) {
        (Some(access_key_id), Some(secret_access_key)) => {
            Some(Credentials { access_key_id, secret_access_key, session_token: get("aws_session_token") })
        }
        _ => None,
    };
    Profile { credentials, region: get("region"), endpoint: get("endpoint_url") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aws_bucket_from_every_host_form() {
        let b = |host: &str, rest: &str| aws_bucket(host, rest);
        assert_eq!(b("data.s3.amazonaws.com", "data.s3.amazonaws.com/a/b.tif").as_deref(), Some("data"));
        assert_eq!(b("data.s3.us-west-2.amazonaws.com", "x/a.tif").as_deref(), Some("data"));
        // The legacy regional form: the bucket is the host's, never a key's first segment.
        assert_eq!(b("data.s3-us-west-2.amazonaws.com", "data.s3-us-west-2.amazonaws.com/other/a.tif").as_deref(), Some("data"));
        // Path style: the first path segment.
        assert_eq!(b("s3-us-west-2.amazonaws.com", "s3-us-west-2.amazonaws.com/data/a.tif").as_deref(), Some("data"));
        assert_eq!(b("s3.amazonaws.com", "s3.amazonaws.com/data/a.tif").as_deref(), Some("data"));
    }

    fn example() -> Credentials {
        Credentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
            session_token: None,
        }
    }

    #[test]
    fn signature_v4_test_vectors() {
        // AWS's Signature Version 4 test suite, "get-vanilla".
        let cr = Credentials {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            session_token: None,
        };
        let h = vec![("host".to_string(), "example.amazonaws.com".to_string()), ("x-amz-date".to_string(), "20150830T123600Z".to_string())];
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let r = Request { method: "GET", path: "/", query: "", headers: &h, payload_hash: empty };
        assert_eq!(
            authorization(&r, "us-east-1", "service", "20150830T123600Z", &cr),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
        // The S3 documentation's GET Object example (a range of examplebucket/test.txt).
        let h = vec![
            ("host".to_string(), "examplebucket.s3.amazonaws.com".to_string()),
            ("range".to_string(), "bytes=0-9".to_string()),
            ("x-amz-content-sha256".to_string(), empty.to_string()),
            ("x-amz-date".to_string(), "20130524T000000Z".to_string()),
        ];
        let r = Request { method: "GET", path: "/test.txt", query: "", headers: &h, payload_hash: empty };
        assert!(
            authorization(&r, "us-east-1", "s3", "20130524T000000Z", &example())
                .ends_with("Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41")
        );
        // ... and its GET Bucket (list objects) example: the query in canonical order.
        let h = vec![
            ("host".to_string(), "examplebucket.s3.amazonaws.com".to_string()),
            ("x-amz-content-sha256".to_string(), empty.to_string()),
            ("x-amz-date".to_string(), "20130524T000000Z".to_string()),
        ];
        let r = Request { method: "GET", path: "/", query: "prefix=J&max-keys=2", headers: &h, payload_hash: empty };
        assert!(
            authorization(&r, "us-east-1", "s3", "20130524T000000Z", &example())
                .ends_with("Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7")
        );
    }

    #[test]
    fn dates_and_encoding() {
        assert_eq!(amz_date(0), "19700101T000000Z");
        assert_eq!(amz_date(1_369_353_600), "20130524T000000Z");
        assert_eq!(amz_date(1_709_251_199), "20240229T235959Z");
        assert_eq!(encode("a b/c~d+é", false), "a%20b/c~d%2B%C3%A9");
        assert_eq!(encode("a/b", true), "a%2Fb");
        assert_eq!(decode("a%2Fb+c%zz"), "a/b c%zz");
        assert_eq!(canonical_query("b=2&a=x%20y&a=1"), "a=1&a=x%20y&b=2");
        assert_eq!(format!("{:?}", example()), "Credentials(…)");
    }

    #[test]
    fn addresses() {
        assert_eq!(parse("s3://my-bucket/a/b c.tif").unwrap(), ("my-bucket".into(), "a/b c.tif".into()));
        assert_eq!(parse("S3://my-bucket").unwrap(), ("my-bucket".into(), String::new()));
        for bad in ["s3://", "s3://A_B/x", "s3://ab/x", "https://x.org/a", "s3://-ab-/x"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert_eq!(object_url("my-bucket", "a/b c.tif", None), "https://my-bucket.s3.amazonaws.com/a/b%20c.tif");
        assert_eq!(object_url("my.bucket", "k", None), "https://s3.amazonaws.com/my.bucket/k");
        let aws = Connection { name: "a".into(), endpoint: None, region: "eu-west-1".into(), buckets: vec![], credentials: None };
        assert_eq!(object_url("b-1", "k", Some(&aws)), "https://b-1.s3.eu-west-1.amazonaws.com/k");
        let minio = Connection { endpoint: Some("http://127.0.0.1:9000".into()), ..aws.clone() };
        assert_eq!(object_url("b-1", "k/x", Some(&minio)), "http://127.0.0.1:9000/b-1/k/x");
        assert_eq!(
            list_url("b-1", "a/b c/", None, Some(&minio)),
            "http://127.0.0.1:9000/b-1?list-type=2&delimiter=%2F&max-keys=1000&prefix=a%2Fb%20c%2F"
        );
        assert_eq!(
            list_url("b-1", "", Some("t/1"), None),
            "https://b-1.s3.amazonaws.com/?list-type=2&delimiter=%2F&max-keys=1000&prefix=&continuation-token=t%2F1"
        );
        assert_eq!(split_url("https://h.org:443/a?b=1#x").unwrap(), ("h.org".into(), "/a".into(), "b=1".into()));
        assert_eq!(split_url("http://h:9000").unwrap(), ("h:9000".into(), "/".into(), String::new()));
    }

    #[test]
    fn connections_sign_only_their_own_endpoint() {
        let cr = Some(Credentials { access_key_id: "AKID".into(), secret_access_key: "secret".into(), session_token: Some("tok".into()) });
        connect(Connection {
            name: "test-sign".into(),
            endpoint: Some("http://127.0.0.1:9999".into()),
            region: String::new(),
            buckets: vec!["sign-test".into()],
            credentials: cr.clone(),
        })
        .unwrap();
        let p = prepare("s3://sign-test/x/y.tif").unwrap();
        assert_eq!(p.url, "http://127.0.0.1:9999/sign-test/x/y.tif");
        assert!(p.signed);
        let names: Vec<&str> = p.headers.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["x-amz-content-sha256", "x-amz-date", "x-amz-security-token", "authorization"]);
        assert!(p.headers.iter().all(|(_, v)| !v.contains("secret")));
        // The same endpoint by address is signed; another host, or the same with a user part, isn't.
        assert!(prepare("http://127.0.0.1:9999/sign-test/x").unwrap().signed);
        for other in ["http://127.0.0.1:9998/sign-test/x", "https://127.0.0.1:9999/x", "http://u@127.0.0.1:9999/x", "https://example.org/x"]
        {
            let p = prepare(other).unwrap();
            assert!(!p.signed && p.headers.is_empty() && p.url == other, "{other}");
        }
        // A bucket no connection names is anonymous at AWS's global endpoint (unless another test's
        // default connection is there).
        assert!(disconnect("test-sign") && !disconnect("test-sign"));
        assert!(
            connect(Connection { name: " ".into(), endpoint: None, region: String::new(), buckets: vec![], credentials: None }).is_err()
        );
        assert!(
            connect(Connection {
                name: "x".into(),
                endpoint: Some("ftp://h".into()),
                region: String::new(),
                buckets: vec![],
                credentials: None
            })
            .is_err()
        );
        assert!(
            connect(Connection {
                name: "x".into(),
                endpoint: Some("https://h.org/path".into()),
                region: String::new(),
                buckets: vec![],
                credentials: None
            })
            .is_err()
        );
        assert!(
            connect(Connection { name: "x".into(), endpoint: None, region: "a b".into(), buckets: vec![], credentials: None }).is_err()
        );
        assert!(
            connect(Connection { name: "x".into(), endpoint: None, region: String::new(), buckets: vec!["UP".into()], credentials: None })
                .is_err()
        );
        assert!(
            connect(Connection {
                name: "x".into(),
                endpoint: Some("http://169.254.169.254".into()),
                region: String::new(),
                buckets: vec![],
                credentials: None
            })
            .is_err()
        );
    }

    #[test]
    fn connections_sign_only_buckets_they_serve() {
        let cr = || Some(Credentials { access_key_id: "AKID2".into(), secret_access_key: "s2".into(), session_token: None });
        // A connection given no buckets claims none: another bucket is read anonymously from AWS.
        connect(Connection {
            name: "rule-endpoint".into(),
            endpoint: Some("https://minio.rule.example:443".into()),
            region: String::new(),
            buckets: vec![],
            credentials: cr(),
        })
        .unwrap();
        assert_eq!(connection("rule-endpoint").unwrap().endpoint.as_deref(), Some("https://minio.rule.example"), "default port dropped");
        let p = prepare("s3://rule-other-bucket/k").unwrap();
        assert_eq!((p.url.as_str(), p.signed), ("https://rule-other-bucket.s3.amazonaws.com/k", false));
        // Its own endpoint is signed, whatever the default port's spelling; the path as S3 encodes it.
        let p = prepare("https://minio.rule.example:443/b-1/a b(1)+c%7e.tif").unwrap();
        assert!(p.signed);
        assert_eq!(p.url, "https://minio.rule.example:443/b-1/a%20b%281%29%2Bc~.tif");
        // A bucket browsed through it is served from then on.
        grant("rule-endpoint", "rule-granted");
        let p = prepare("s3://rule-granted/k").unwrap();
        assert_eq!((p.url.as_str(), p.signed), ("https://minio.rule.example/rule-granted/k", true));
        // ... unless read anonymously on purpose.
        let p = prepare(&anonymous("s3://rule-granted/k")).unwrap();
        assert_eq!((p.url.as_str(), p.signed), ("https://rule-granted.s3.amazonaws.com/k", false));
        assert!(crate::address::check(&anonymous("s3://rule-granted/k"), None).is_ok());
        // An AWS connection signs only its buckets at amazonaws.com, for the region the address names.
        connect(Connection {
            name: "rule-aws".into(),
            endpoint: None,
            region: "eu-west-1".into(),
            buckets: vec!["rule-mine".into()],
            credentials: cr(),
        })
        .unwrap();
        let p = prepare("https://rule-mine.s3.us-west-2.amazonaws.com/k").unwrap();
        assert!(p.signed && p.headers.iter().any(|(k, v)| k == "authorization" && v.contains("/us-west-2/s3/aws4_request")));
        let p = prepare("https://rule-mine.s3.amazonaws.com/k").unwrap();
        assert!(p.headers.iter().any(|(k, v)| k == "authorization" && v.contains("/eu-west-1/s3/aws4_request")));
        for other in [
            "https://rule-theirs.s3.amazonaws.com/k",
            "https://s3.us-east-1.amazonaws.com/rule-theirs/k",
            "https://rule-mine.s3.amazonaws.com.evil.example/k",
            "http://rule-mine.s3.amazonaws.com/k",
        ] {
            assert!(!prepare(other).unwrap().signed, "{other}");
        }
        // The account's bucket list (the service root of its region) is signed; another region's
        // root, and the root with a path, aren't.
        let c = connection("rule-aws").unwrap();
        let p = prepare(&buckets_url(&c)).unwrap();
        assert!(p.signed && p.headers.iter().any(|(k, v)| k == "authorization" && v.contains("/eu-west-1/s3/aws4_request")));
        assert!(!prepare("https://s3.us-east-1.amazonaws.com/").unwrap().signed);
        // Replacing a connection by name forgets what was granted to the old one.
        connect(Connection {
            name: "rule-endpoint".into(),
            endpoint: Some("https://other.rule.example".into()),
            region: String::new(),
            buckets: vec![],
            credentials: cr(),
        })
        .unwrap();
        assert!(!prepare("s3://rule-granted/k").unwrap().signed, "grants don't pass to a replacement");
        disconnect("rule-endpoint");
        disconnect("rule-aws");
        assert!(!prepare("s3://rule-granted/k").unwrap().signed, "grants go with the connection");
    }

    #[test]
    fn listings_and_profiles() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>b</Name><Prefix>tiles/</Prefix><IsTruncated>true</IsTruncated><NextContinuationToken>abc==</NextContinuationToken>
<Contents><Key>tiles/a.tif</Key><LastModified>2024-01-02T03:04:05.000Z</LastModified><Size>1234</Size></Contents>
<CommonPrefixes><Prefix>tiles/2024/</Prefix></CommonPrefixes></ListBucketResult>"#;
        let l = parse_list(xml).unwrap();
        assert_eq!(l.objects, vec![Object { key: "tiles/a.tif".into(), size: 1234, modified: "2024-01-02T03:04:05.000Z".into() }]);
        assert_eq!((l.prefixes, l.next), (vec!["tiles/2024/".to_string()], Some("abc==".to_string())));
        let err = br#"<Error><Code>AccessDenied</Code><Message>Access Denied</Message></Error>"#;
        assert_eq!(parse_list(err).unwrap_err(), "AccessDenied: Access Denied");
        assert!(parse_list(b"<html>").is_err() && parse_list(b"<a/>").is_err() && parse_list(&[0xff]).is_err());
        let buckets = br#"<ListAllMyBucketsResult><Buckets><Bucket><Name>one</Name></Bucket><Bucket><Name>BAD_</Name></Bucket></Buckets></ListAllMyBucketsResult>"#;
        assert_eq!(parse_buckets(buckets).unwrap(), vec!["one".to_string()]);
        let creds = "[default]\naws_access_key_id = A1\naws_secret_access_key=S1\n[work]\naws_access_key_id=A2\naws_secret_access_key = S2\naws_session_token=T2\n";
        let config = "[default]\nregion = eu-central-1\n[profile work]\nregion=us-west-2\nendpoint_url = https://minio.example.org\n";
        let d = profile(creds, config, "default");
        assert_eq!((d.credentials.unwrap().access_key_id, d.region, d.endpoint), ("A1".into(), Some("eu-central-1".into()), None));
        let w = profile(creds, config, "work");
        let wc = w.credentials.unwrap();
        assert_eq!(
            (wc.secret_access_key.as_str(), wc.session_token.as_deref(), w.endpoint.as_deref()),
            ("S2", Some("T2"), Some("https://minio.example.org"))
        );
        assert_eq!(profile(creds, config, "none"), Profile::default());
    }
}
