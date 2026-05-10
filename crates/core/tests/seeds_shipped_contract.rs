//! Contract tests for the shipped `data/seeds/*.json` packs.
//!
//! Mirrors `catalog_shipped_contract.rs`: every shipped pack must (a)
//! parse and validate, (b) name an app id that exists in the shipped
//! catalog, (c) preserve historically-required keys for the three apps
//! the manager has shipped seed support for since 2026-04-16.

mod common;

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;

use appimage_control_core::{Catalog, SeedPackLibrary};

/// Read every `data/seeds/*.json` file as a (stem, raw) pair.
fn shipped_packs() -> Vec<(String, String)> {
    let dir = common::shipped_seeds_dir();
    let mut packs = Vec::new();
    for entry in fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
    {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(OsStr::to_str) != Some("json") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap()
            .to_string();
        let raw = fs::read_to_string(&path).unwrap();
        packs.push((stem, raw));
    }
    packs.sort_by(|a, b| a.0.cmp(&b.0));
    packs
}

#[test]
fn every_shipped_pack_parses_and_validates() {
    let packs = shipped_packs();
    assert!(
        packs.len() >= 3,
        "expected at least 3 shipped packs (feather, electrum-btc, electrum-ltc); got {}",
        packs.len()
    );

    // Round-trip via the same loader the binary uses at startup so a
    // bad pack fails this test the same way it fails launch.
    let lib = SeedPackLibrary::from_pairs(
        packs.iter().map(|(id, raw)| (id.as_str(), raw.as_str())),
    )
    .expect("shipped packs must parse");
    lib.validate().expect("shipped packs must validate clean");
}

#[test]
fn every_shipped_pack_id_is_in_the_catalog() {
    let path = common::shipped_catalog_path();
    let catalog = Catalog::load_from_path(&path).expect("load catalog");
    let catalog_ids: HashSet<&str> =
        catalog.apps.iter().map(|a| a.id.as_str()).collect();

    for (pack_id, _) in shipped_packs() {
        assert!(
            catalog_ids.contains(pack_id.as_str()),
            "shipped seed pack `{pack_id}.json` has no matching catalog app id; \
             rename the file or add the app to data/catalog.json"
        );
    }
}

#[test]
fn feather_pack_keeps_historic_keys() {
    let raw = fs::read_to_string(common::shipped_seeds_dir().join("feather.json"))
        .expect("read feather.json");
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();

    // Static keys carry their non-data-driven invariants:
    let st = v.get("static").and_then(|s| s.as_object()).expect("static block");
    assert_eq!(st.get("proxy"), Some(&serde_json::json!(1)));
    assert_eq!(st.get("torOnlyAllowOnion"), Some(&serde_json::json!(true)));
    assert_eq!(st.get("torPrivacyLevel"), Some(&serde_json::json!(2)));

    // Preset block must still drive useLocalTor + socks5Host (with the
    // drop_if guard against double-binding when local Tor is on).
    let pr = v.get("preset").and_then(|s| s.as_object()).expect("preset block");
    assert!(pr.contains_key("useLocalTor"));
    assert!(pr.contains_key("socks5Host"));

    // Skin rotation must still be a 3-entry list (1-based instance %3).
    let rot = v.get("rotation").and_then(|s| s.as_object()).expect("rotation block");
    let skin_values = rot
        .get("skin")
        .and_then(|s| s.get("values"))
        .and_then(|v| v.as_array())
        .expect("skin.values array");
    assert_eq!(skin_values.len(), 3, "feather rotates 3 skins");

    // socks5Port must be string-coerced (matches Feather's
    // settings.json schema where port is a string).
    let port = rot.get("socks5Port").expect("socks5Port rotation");
    assert_eq!(
        port.get("from_preset_array"),
        Some(&serde_json::json!("feather_ports"))
    );
    assert_eq!(port.get("as_string"), Some(&serde_json::json!(true)));
}

#[test]
fn electrum_btc_pack_pins_an_onion_and_bookmarks_at_least_nine() {
    let raw = fs::read_to_string(common::shipped_seeds_dir().join("electrum-btc.json"))
        .expect("read electrum-btc.json");
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let st = v.get("static").and_then(|s| s.as_object()).expect("static block");

    let server = st
        .get("server")
        .and_then(|s| s.as_str())
        .expect("server entry");
    assert!(
        server.ends_with(":50002:s") && server.contains(".onion:"),
        "BTC pinned server should be a .onion:50002:s endpoint, got {server}"
    );

    let bookmarked = st
        .get("network_bookmarked_servers")
        .and_then(|s| s.as_array())
        .expect("network_bookmarked_servers array");
    assert!(
        bookmarked.len() >= 9,
        "expected at least 9 bookmarked Electrum BTC servers, got {}",
        bookmarked.len()
    );
    for s in bookmarked {
        let s = s.as_str().expect("bookmarked entry must be a string");
        assert!(s.contains(".onion:"), "bookmarked {s} must be an onion endpoint");
    }
}

#[test]
fn electrum_ltc_pack_uses_correct_proxy_template() {
    let raw = fs::read_to_string(common::shipped_seeds_dir().join("electrum-ltc.json"))
        .expect("read electrum-ltc.json");
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let proxy = v
        .pointer("/preset/proxy/format")
        .and_then(|s| s.as_str())
        .expect("preset.proxy.format string");
    assert!(
        proxy.contains("{electrum_ltc_port}") && proxy.contains("{socks5_host}"),
        "LTC proxy template must reference both socks5_host and electrum_ltc_port; got {proxy}"
    );
}
