//! The JSON Schemas in spec/schema and the Rust parser must agree on every fixture.

use std::fs;
use std::path::PathBuf;

fn spec() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../spec")
}

fn validator(name: &str) -> jsonschema::Validator {
    let s: serde_json::Value = serde_json::from_slice(&fs::read(spec().join("schema").join(name)).unwrap()).unwrap();
    jsonschema::validator_for(&s).unwrap()
}

fn fixtures(kind: &str) -> Vec<(String, Vec<u8>)> {
    let dir = spec().join("fixtures/manifest").join(kind);
    let mut v: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            (p.file_name().unwrap().to_string_lossy().to_string(), fs::read(&p).unwrap())
        })
        .collect();
    v.sort();
    assert!(!v.is_empty(), "no fixtures in {}", dir.display());
    v
}

#[test]
fn valid_manifests_pass_schema_and_core() {
    let v = validator("manifest.schema.json");
    for (name, bytes) in fixtures("valid") {
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let errs: Vec<String> = v.iter_errors(&j).map(|e| e.to_string()).collect();
        assert!(errs.is_empty(), "{name}: schema rejected: {errs:?}");
        andysu_core::manifest::Manifest::parse(&bytes).unwrap_or_else(|e| panic!("{name}: core rejected: {e}"));
    }
}

#[test]
fn invalid_manifests_fail_schema_and_core() {
    let v = validator("manifest.schema.json");
    for (name, bytes) in fixtures("invalid") {
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(!v.is_valid(&j), "{name}: schema accepted an invalid manifest");
        assert!(andysu_core::manifest::Manifest::parse(&bytes).is_err(), "{name}: core accepted an invalid manifest");
    }
}

#[test]
fn core_output_validates() {
    // Round-trip: what the core serialises is valid under the schema.
    let v = validator("manifest.schema.json");
    for (name, bytes) in fixtures("valid") {
        let m = andysu_core::manifest::Manifest::parse(&bytes).unwrap();
        let j = serde_json::to_value(&m).unwrap();
        assert!(v.is_valid(&j), "{name}: re-serialised manifest invalid");
    }
    let st = andysu_core::state::State::new("0d5e2f8a-1b3c-4d5e-8f70-123456789abc".into());
    assert!(validator("state.schema.json").is_valid(&serde_json::to_value(&st).unwrap()));
}

#[test]
fn payload_descriptor_schema() {
    let v = validator("payload.schema.json");
    let good = serde_json::json!({"schema":1,"app":"com.x","version":"1.0","host_api_min":1,
        "entry":{"native_lib":"libgame.so","symbol":"andysu_payload_v1"},"needs":{"permissions":["android.permission.INTERNET"]}});
    assert!(v.is_valid(&good));
    andysu_core::payload::Descriptor::parse(good.to_string().as_bytes()).unwrap();
    for bad in [
        serde_json::json!({"schema":1,"app":"com.x","version":"1.0","host_api_min":1,"entry":{}}),
        serde_json::json!({"schema":1,"app":"com.x","version":"1.0","host_api_min":0,"entry":{"dex_class":"a.B"}}),
        serde_json::json!({"schema":1,"app":"com.x","version":"1.0","host_api_min":1,"entry":{"native_lib":"libx.so"}}),
    ] {
        assert!(!v.is_valid(&bad), "{bad}");
        assert!(andysu_core::payload::Descriptor::parse(bad.to_string().as_bytes()).is_err(), "{bad}");
    }
}
