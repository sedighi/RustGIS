//! Source Cooperative (https://source.coop): open data products by their publishers. Its API lists
//! the featured products and an account's; a product's files are listed through its S3-compatible
//! data proxy (`data.source.coop`, the account as the bucket, the product as the first folder)
//! and read by their web address there (GeoParquet, FlatGeobuf and cloud-optimised GeoTIFFs in
//! place, by range). Requests are built and answers read here, the fetching is the caller's.

use serde_json::Value;

/// The products API.
pub const API: &str = "https://source.coop/api/v1/products";
/// The data proxy.
pub const DATA: &str = "https://data.source.coop";
/// The most products an answer is read for.
const MAX_PRODUCTS: usize = 1000;

/// An account's or a product's id, checked: it goes into addresses.
pub fn id(s: &str) -> Result<String, String> {
    let s = s.trim();
    if !s.is_empty()
        && s.len() <= 100
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        && !s.starts_with('.')
    {
        Ok(s.to_string())
    } else {
        Err(format!("'{s}' isn't a Source Cooperative account or product"))
    }
}

/// The featured products.
pub fn featured_url() -> String {
    format!("{API}/featured")
}

/// An account's products.
pub fn account_url(account: &str) -> Result<String, String> {
    Ok(format!("{API}/{}", id(account)?))
}

/// A product, as the API describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Product {
    pub account: String,
    pub id: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
}

/// The products of an answer (public ones whose ids are sound).
pub fn parse_products(bytes: &[u8]) -> Result<Vec<Product>, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| "Source Cooperative's answer isn't JSON".to_string())?;
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Err(format!("Source Cooperative: {}", e.chars().take(200).collect::<String>()));
    }
    let list = v.get("products").and_then(Value::as_array).ok_or("Source Cooperative's answer lists no products")?;
    let text = |p: &Value, k: &str, n: usize| p.get(k).and_then(Value::as_str).unwrap_or("").chars().take(n).collect::<String>();
    Ok(list
        .iter()
        .take(MAX_PRODUCTS)
        .filter(|p| p.get("visibility").and_then(Value::as_str).is_none_or(|v| v == "public"))
        .filter(|p| p.get("disabled").and_then(Value::as_bool) != Some(true))
        .filter_map(|p| {
            let account = id(p.get("account_id")?.as_str()?).ok()?;
            let pid = id(p.get("product_id")?.as_str()?).ok()?;
            let tags = p
                .pointer("/metadata/tags")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(20)
                .filter_map(Value::as_str)
                .map(|t| t.chars().take(40).collect())
                .collect();
            let title = Some(text(p, "title", 200)).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| pid.clone());
            Some(Product { account, id: pid, title, description: text(p, "description", 600), tags })
        })
        .collect())
}

/// The request listing a level of a product: `path` is inside the product (empty for its top,
/// else ending in `/`).
pub fn list_url(account: &str, product: &str, path: &str, token: Option<&str>) -> Result<String, String> {
    let (account, product) = (id(account)?, id(product)?);
    if path.contains("..") || path.starts_with('/') || path.chars().any(char::is_control) || path.len() > 1024 {
        return Err(format!("'{path}' isn't a folder of the product"));
    }
    let prefix = format!("{product}/{path}");
    let mut q = format!("{DATA}/{account}/?list-type=2&delimiter=%2F&max-keys=1000&prefix={}", crate::s3::encode(&prefix, true));
    if let Some(t) = token {
        q.push_str(&format!("&continuation-token={}", crate::s3::encode(t, true)));
    }
    Ok(q)
}

/// A file's web address on the data proxy (`key` as a listing gives it: the product first).
pub fn file_url(account: &str, key: &str) -> Result<String, String> {
    let account = id(account)?;
    if key.is_empty() || key.contains("..") || key.starts_with('/') || key.chars().any(char::is_control) || key.len() > 1024 {
        return Err(format!("'{key}' isn't a file of the product"));
    }
    Ok(format!("{DATA}/{account}/{}", crate::s3::encode(key, false)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn products_and_addresses() {
        let a = br#"{"products":[
          {"account_id":"giswqs","product_id":"opengeos","title":"Open Geospatial Data","description":"For public use.","visibility":"public","disabled":false,"metadata":{"tags":["vector","parquet"]}},
          {"account_id":"x","product_id":"hidden","title":"H","visibility":"restricted"},
          {"account_id":"../etc","product_id":"p","title":"Bad"},
          {"account_id":"y","product_id":"off","disabled":true},
          {"account_id":"z","product_id":"untitled","title":"  "}]}"#;
        let p = parse_products(a).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(
            p[0],
            Product {
                account: "giswqs".into(),
                id: "opengeos".into(),
                title: "Open Geospatial Data".into(),
                description: "For public use.".into(),
                tags: vec!["vector".into(), "parquet".into()]
            }
        );
        assert_eq!(p[1].title, "untitled");
        assert!(parse_products(br#"{"error":"No API route"}"#).unwrap_err().contains("No API route"));
        assert!(parse_products(b"<html>").is_err() && parse_products(b"{}").is_err());

        assert_eq!(featured_url(), "https://source.coop/api/v1/products/featured");
        assert_eq!(account_url("giswqs").unwrap(), "https://source.coop/api/v1/products/giswqs");
        assert!(account_url("a/b").is_err() && account_url("").is_err() && account_url("..").is_err());
        assert_eq!(
            list_url("giswqs", "opengeos", "", None).unwrap(),
            "https://data.source.coop/giswqs/?list-type=2&delimiter=%2F&max-keys=1000&prefix=opengeos%2F"
        );
        assert!(
            list_url("giswqs", "opengeos", "EMIT/", Some("t+k="))
                .unwrap()
                .ends_with("prefix=opengeos%2FEMIT%2F&continuation-token=t%2Bk%3D")
        );
        assert!(list_url("giswqs", "opengeos", "../x/", None).is_err());
        assert_eq!(file_url("giswqs", "opengeos/a b/c.parquet").unwrap(), "https://data.source.coop/giswqs/opengeos/a%20b/c.parquet");
        assert!(file_url("giswqs", "/etc/passwd").is_err() && file_url("giswqs", "a/../b").is_err());
    }
}
