//! Deterministic OGC documents generated in memory by the data, engine and UI tests.
//! `cargo test --workspace --locked` needs no downloaded fixtures or generation step.
#![cfg(test)]
#![allow(dead_code)]

use serde_json::{Value, json};
use std::sync::LazyLock;

pub fn fixture_landing() -> &'static str {
    static TEXT: LazyLock<String> = LazyLock::new(|| {
        json!({
            "title": "Example GIS",
            "links": [
                {"rel":"data", "type":"text/html", "href":"/ogc/user/test-token/catalog.html"},
                {"rel":"data", "type":"application/json", "href":"/ogc/user/test-token/catalog?f=json&key=fixture-key"}
            ]
        })
        .to_string()
    });
    &TEXT
}

fn collection(id: &str, title: &str) -> Value {
    json!({"id":id, "title":title, "links":[{
        "rel":"items", "type":"application/geo+json", "href":format!("collections/{id}/items?key=fixture-key")
    }]})
}

pub fn fixture_collections() -> &'static str {
    static TEXT: LazyLock<String> = LazyLock::new(|| {
        let mut places = collection("places", "Places");
        places["extent"] = json!({"spatial":{"bbox":[[10,50,10,50]]}});
        json!({"title":"Example GIS", "collections":[places]}).to_string()
    });
    &TEXT
}

pub fn fixture_group_collections() -> &'static str {
    static TEXT: LazyLock<String> =
        LazyLock::new(|| json!({"collections":[collection("places", "Places"), collection("routes", "Routes")]}).to_string());
    &TEXT
}

fn feature(id: &str, name: &str, geometry: Value) -> String {
    json!({"type":"FeatureCollection", "features":[{
        "type":"Feature", "id":id, "properties":{"name":name}, "geometry":geometry
    }]})
    .to_string()
}

pub fn fixture_features() -> &'static str {
    static TEXT: LazyLock<String> = LazyLock::new(|| feature("place-1", "Test Place", json!({"type":"Point", "coordinates":[10,50]})));
    &TEXT
}

pub fn fixture_lines() -> &'static str {
    static TEXT: LazyLock<String> =
        LazyLock::new(|| feature("route-1", "Test Route", json!({"type":"LineString", "coordinates":[[10.0,50.0],[11.0,51.0]]})));
    &TEXT
}
