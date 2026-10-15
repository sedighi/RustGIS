//! PostgreSQL connection strings (libpq's `key=value` form and URIs): taken apart by libpq's
//! rules, so a connection can be named without its secrets. Pure text, so the browser build has
//! it too.

/// Keys of a libpq connection string or URI query that are secrets, or point at one, and are never
/// kept in a label.
const SECRET_KEYS: &[&str] = &["password", "sslpassword", "passfile"];

/// A PostgreSQL connection, as written to be kept (a list of sources, a log): `postgres:<keyword
/// string>` or a `postgresql://` URI, without its password or any other secret, and with no side
/// effect (unlike `duckdb::Database::parse`, nothing is remembered). Takes `postgres:`, `postgresql:`,
/// GDAL's `PG:` and the URI forms; `None` for anything else.
pub fn public_label(s: &str) -> Option<String> {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("postgresql://") || lower.starts_with("postgres://") {
        let (no_userinfo, _) = split_secrets(t, &["password"]);
        // A secret in the query (`?password=…`) goes too.
        return Some(match no_userinfo.split_once('?') {
            Some((base, q)) => {
                let kept: Vec<&str> = q
                    .split('&')
                    .filter(|kv| {
                        let k = kv.split_once('=').map(|(k, _)| k).unwrap_or(kv);
                        !kv.is_empty() && !SECRET_KEYS.iter().any(|s| k.eq_ignore_ascii_case(s))
                    })
                    .collect();
                if kept.is_empty() { base.to_string() } else { format!("{base}?{}", kept.join("&")) }
            }
            None => no_userinfo,
        });
    }
    let rest = ["postgresql:", "postgres:", "pg:"].iter().find(|p| lower.starts_with(**p)).and_then(|p| t.get(p.len()..))?;
    let (public, _) = split_secrets(rest.trim(), SECRET_KEYS);
    // Nothing that names a database (`postgres:dbname`, `PG:`): not a connection to keep.
    if public.is_empty() {
        return None;
    }
    Some(format!("postgres:{public}"))
}

/// Where a URI's user and password end (the `@` before the host), given what follows `://`; `None`
/// when it has none. Normally the authority runs to the first `/`, `?` or `#`, and an `@` in it
/// ends the userinfo. A password holding an unencoded `/`, `?`, `#` or `@` (malformed, but it
/// happens) ends the authority early: when the authority so cut has no `@` but a `:` that isn't
/// a port's (digits only), the last `@` ends the userinfo, so the password is never kept. A real
/// port (`host:8080/@user`) is left alone.
pub fn userinfo_end(rest: &str) -> Option<usize> {
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = rest.get(..end)?;
    if let Some(i) = authority.rfind('@') {
        return Some(i);
    }
    let (_, after) = authority.split_once(':')?;
    // `host:8080` (or an IPv6 literal's colons) is a host and port, not a user and password.
    // A port: empty (`host:`), digits, or digits then another host (libpq's `h1:5432,h2:5432`).
    let digits = after.bytes().take_while(u8::is_ascii_digit).count();
    let port = after.is_empty() || digits == after.len() || (digits > 0 && after.as_bytes().get(digits) == Some(&b','));
    if authority.starts_with('[') || port {
        return None;
    }
    // A malformed authority: the password may hold any of `/?#@`, so the last `@` ends it.
    rest.rfind('@').filter(|&i| i >= end)
}

/// A connection string without the values of `secrets` (the first, `password`, returned).
pub(crate) fn split_secrets(dsn: &str, secrets: &[&str]) -> (String, Option<String>) {
    let lower = dsn.to_ascii_lowercase();
    if lower.starts_with("postgresql://") || lower.starts_with("postgres://") {
        // scheme://[user[:password]@]host…: the credentials are in the authority, before the
        // path, query or fragment (an `@` there is something else).
        let Some((scheme, rest)) = dsn.split_once("://") else { return (dsn.to_string(), None) };
        // A PostgreSQL URI's path is only the database's name, so an `@` after the authority and
        // before any query ends a password that held an unencoded `/` (even one of digits only,
        // which reads like a port: `u:123/w@db`). Found on the whole text: a password may hold
        // a `#` too.
        let at = userinfo_end(rest).or_else(|| {
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            let query = rest.find('?').unwrap_or(rest.len());
            rest.get(..query)?.rfind('@').filter(|&i| i >= end)
        });
        // A connection's fragment is never kept (it isn't part of a connection).
        let no_fragment = |s: &str| s.split('#').next().unwrap_or(s).to_string();
        if let Some(at) = at
            && let (Some(userinfo), Some(after)) = (rest.get(..at), rest.get(at + 1..))
        {
            let after = no_fragment(after);
            return match userinfo.split_once(':') {
                Some((user, password)) => (format!("{scheme}://{user}@{after}"), Some(password.to_string())),
                None => (format!("{scheme}://{userinfo}@{after}"), None),
            };
        }
        return (format!("{scheme}://{}", no_fragment(rest)), None);
    }
    // libpq's keyword = value pairs; a value is a word, or quoted with \' and \\ inside.
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut password = None;
    let mut chars = dsn.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.peek().is_none() {
            break;
        }
        let mut key = String::new();
        while let Some(c) = chars.next_if(|c| !c.is_whitespace() && *c != '=') {
            key.push(c);
        }
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.next() != Some('=') {
            // Not a connection string RustGIS can take apart: what came before it (never the
            // rest, which could hold a password).
            break;
        }
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let mut value = String::new();
        if chars.next_if_eq(&'\'').is_some() {
            loop {
                match chars.next() {
                    Some('\\') => value.extend(chars.next()),
                    Some('\'') => break,
                    Some(c) => value.push(c),
                    // An unterminated quote: the rest is dropped (it could be a password).
                    None => {
                        key.clear();
                        break;
                    }
                }
            }
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                if c == '\\' {
                    value.extend(chars.next());
                } else {
                    value.push(c);
                }
            }
        }
        if key.is_empty() {
            break;
        }
        if key.eq_ignore_ascii_case("password") {
            password = Some(value);
        } else if !secrets.iter().any(|s| key.eq_ignore_ascii_case(s)) {
            pairs.push((key, value));
        }
    }
    let quote = |v: &str| {
        if !v.is_empty() && !v.contains([' ', '\t', '\'', '\\']) {
            v.to_string()
        } else {
            format!("'{}'", v.replace('\\', "\\\\").replace('\'', "\\'"))
        }
    };
    (pairs.iter().map(|(k, v)| format!("{k}={}", quote(v))).collect::<Vec<_>>().join(" "), password)
}
