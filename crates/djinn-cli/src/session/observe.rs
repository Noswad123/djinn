//! Read-only, persisted evidence. This module deliberately does not use session resolvers.
use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use anyhow::{anyhow, Result};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::cli_args::SessionObserveArgs;

#[cfg(unix)]
mod fs_unix;

const MANIFEST_LIMIT: usize = 65_536;
const EVENTS_LIMIT: usize = 4_194_304;
const LINE_LIMIT: usize = 65_536;
const RECORD_LIMIT: usize = 16_384;
type EvidenceResult<T> = std::result::Result<T, &'static str>;

pub(crate) fn session_observe(args: SessionObserveArgs) -> Result<()> {
    let report = observe(&args.dir)?;
    let output = if args.json {
        serde_json::to_string(&report)?
    } else {
        serde_json::to_string_pretty(&report)?
    };
    println!("{output}");
    Ok(())
}

fn selector(path: &Path) -> Result<&str> {
    let invalid = || {
        anyhow!("observe requires an absolute UTF-8 folder path without parent components, controls, or trailing slash (maximum 4096 bytes)")
    };
    let text = path.to_str().ok_or_else(invalid)?;
    if text.is_empty()
        || text.len() > 4096
        || !path.is_absolute()
        || text.ends_with('/')
        || text.chars().any(char::is_control)
        || text.split('/').any(|part| part == "..")
    {
        return Err(invalid());
    }
    Ok(text)
}

fn observe(path: &Path) -> Result<Value> {
    let selected = selector(path)?;
    let started = chrono::Utc::now().to_rfc3339();
    #[cfg(unix)]
    let result = fs_unix::read(path).and_then(|snapshot| {
        let observation = project(&snapshot.manifest, snapshot.events.as_deref())?;
        Ok((snapshot.resolved, observation))
    });
    #[cfg(not(unix))]
    let result: EvidenceResult<(String, Value)> = Err("unsupported_platform");
    let (assessment, resolved, observation, diagnostics) = match result {
        Ok((resolved, observation)) => ("observed", Some(resolved), Some(observation), vec![]),
        Err(code) => ("unavailable", None, None, vec![code]),
    };
    Ok(json!({
        "schema_version": 1,
        "contract": "djinn.session.observation.v1",
        "assessment": assessment,
        "selected_folder": selected,
        "resolved_folder": resolved,
        "started_at": started,
        "observed_at": chrono::Utc::now().to_rfc3339(),
        "capabilities": {"persisted_folder_events": true, "live_status": false,
            "ui_sessions": false, "native_store": false},
        "authorization": "none", "dispatchable": false,
        "runtime_fact_freshness": "unknown", "native_lifecycle": "unestablished",
        "job_completion": "unestablished", "activity": "unknown",
        "diagnostics": diagnostics, "observation": observation
    }))
}

fn native_id(value: &str) -> bool {
    value.len() <= 128
        && value.strip_prefix("agt_").is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}

fn timestamp(value: &str) -> bool {
    value.len() <= 64 && chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

fn optional_string<'a>(object: &'a Value, key: &str) -> EvidenceResult<Option<&'a str>> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.is_empty() => Ok(Some(value)),
        _ => Err("invalid_metadata"),
    }
}

fn check_identity<'a>(id: &'a str, identity: &mut Option<String>) -> EvidenceResult<&'a str> {
    if !native_id(id) {
        return Err("invalid_native_id");
    }
    if identity.as_deref().is_some_and(|expected| expected != id) {
        return Err("native_id_mismatch");
    }
    if identity.is_none() {
        *identity = Some(id.to_owned());
    }
    Ok(id)
}

fn project(manifest: &[u8], events: Option<&[u8]>) -> EvidenceResult<Value> {
    if manifest.len() > MANIFEST_LIMIT {
        return Err("manifest_limit");
    }
    let document = std::str::from_utf8(manifest)
        .map_err(|_| "invalid_manifest")?
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| "invalid_manifest")?;
    let field = |key| -> EvidenceResult<Option<&str>> {
        document
            .get(key)
            .map(|item| item.as_str().ok_or("invalid_metadata"))
            .transpose()
    };
    let native = field("session_id")?;
    if native.is_some_and(|id| !native_id(id)) {
        return Err("invalid_native_id");
    }
    let created = field("created_at")?;
    if created.is_some_and(|time| !timestamp(time)) {
        return Err("invalid_timestamp");
    }
    let mut identity = native.map(str::to_owned);
    let mut seen = HashSet::new();
    let mut count = 0u64;
    let mut last_event = Value::Null;
    let mut last_lifecycle = Value::Null;
    if let Some(events) = events {
        if events.len() > EVENTS_LIMIT {
            return Err("events_limit");
        }
        if !events.ends_with(b"\n")
            && events
                .rsplit(|b| *b == b'\n')
                .next()
                .is_some_and(|tail| !tail.iter().all(u8::is_ascii_whitespace))
        {
            return Err("incomplete_tail");
        }
        for line in events.split(|b| *b == b'\n') {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            if line.len() > LINE_LIMIT {
                return Err("line_limit");
            }
            count += 1;
            if count > RECORD_LIMIT as u64 {
                return Err("record_limit");
            }
            let mut parser = serde_json::Deserializer::from_slice(line);
            let event = StrictJson(0)
                .deserialize(&mut parser)
                .map_err(|_| "invalid_event_json")?;
            parser.end().map_err(|_| "invalid_event_json")?;
            if !event.is_object() {
                return Err("invalid_metadata");
            }
            if event
                .get("schema_version")
                .is_some_and(|v| v.as_u64() != Some(1))
            {
                return Err("unsupported_event_schema");
            }
            let kind = optional_string(&event, "type")?.ok_or("invalid_kind")?;
            if kind.len() > 64
                || !kind.as_bytes()[0].is_ascii_lowercase()
                || !kind
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                return Err("invalid_kind");
            }
            let event_id = optional_string(&event, "event_id")?;
            if let Some(id) = event_id {
                if id.len() > 256 || id.chars().any(char::is_control) {
                    return Err("invalid_event_id");
                }
                if !seen.insert(id.to_owned()) {
                    return Err("duplicate_event_id");
                }
            }
            let session_id = optional_string(&event, "session_id")?;
            if let Some(id) = session_id {
                check_identity(id, &mut identity)?;
            }
            if kind == "session_created" {
                if let Some(value) = event.get("id") {
                    let id = value.as_str().ok_or("invalid_native_id")?;
                    check_identity(id, &mut identity)?;
                }
            }
            let recorded_at = optional_string(&event, "created_at")?;
            if recorded_at.is_some_and(|time| !timestamp(time)) {
                return Err("invalid_timestamp");
            }
            last_event = json!({"kind": kind, "event_id": event_id,
                "session_id": session_id, "recorded_at": recorded_at});
            if kind == "session_lifecycle_updated" {
                let state = optional_string(&event, "state")?.ok_or("invalid_lifecycle")?;
                if ![
                    "created",
                    "running",
                    "paused",
                    "completed",
                    "failed",
                    "cancelled",
                ]
                .contains(&state)
                {
                    return Err("invalid_lifecycle");
                }
                let mode = optional_string(&event, "mode")?;
                if mode.is_some_and(|mode| !["foreground", "background"].contains(&mode)) {
                    return Err("invalid_lifecycle");
                }
                last_lifecycle = json!({"state": state, "mode": mode, "event_id": event_id,
                    "session_id": session_id, "recorded_at": recorded_at});
            }
        }
    }
    Ok(json!({
        "recorded_native_session_id": native, "recorded_created_at": created,
        "sources": {
            "manifest": {"path": "djinn.toml", "bytes": manifest.len() as u64,
                "sha256": format!("{:x}", Sha256::digest(manifest))},
            "folder_events": {"path": "events.jsonl", "status": if events.is_some() {"observed"} else {"missing"},
                "bytes": events.map(|bytes| bytes.len() as u64),
                "sha256": events.map(|bytes| format!("{:x}", Sha256::digest(bytes)))}
        },
        "event_count": events.map(|_| count), "last_event": last_event,
        "last_lifecycle": last_lifecycle, "consistency": "stable_per_file_not_transactional"
    }))
}

// Validate the entire JSON tree, including payloads that are never projected.
struct StrictJson(usize);

impl<'de> DeserializeSeed<'de> for StrictJson {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for StrictJson {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded JSON")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Value, E> {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("nonfinite"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E: de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> std::result::Result<Value, A::Error> {
        if self.0 >= 32 {
            return Err(de::Error::custom("depth limit"));
        }
        let mut values = Vec::new();
        while let Some(value) = access.next_element_seed(StrictJson(self.0 + 1))? {
            if values.len() == 4096 {
                return Err(de::Error::custom("container limit"));
            }
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> std::result::Result<Value, A::Error> {
        if self.0 >= 32 {
            return Err(de::Error::custom("depth limit"));
        }
        let mut values = Map::new();
        while let Some(key) = access.next_key::<String>()? {
            if values.len() == 4096 || values.contains_key(&key) {
                return Err(de::Error::custom("duplicate key or container limit"));
            }
            values.insert(key, access.next_value_seed(StrictJson(self.0 + 1))?);
        }
        Ok(Value::Object(values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &[u8] =
        b"session_id = 'agt_fixture'\ncreated_at = '2026-10-24T12:00:00+00:00'\n";

    #[test]
    fn manifest_only_and_empty_events_are_distinct() {
        let missing = project(MANIFEST, None).unwrap();
        assert_eq!(missing["recorded_native_session_id"], "agt_fixture");
        assert_eq!(missing["recorded_created_at"], "2026-10-24T12:00:00+00:00");
        assert_eq!(
            missing["sources"]["folder_events"],
            json!({"path":"events.jsonl", "status":"missing", "bytes":null, "sha256":null})
        );
        assert!(missing["event_count"].is_null());
        assert!(missing["last_event"].is_null());
        assert!(missing["last_lifecycle"].is_null());
        let empty = project(MANIFEST, Some(b"")).unwrap();
        assert_eq!(empty["event_count"], 0);
        assert_eq!(empty["sources"]["folder_events"]["bytes"], 0);
        assert_eq!(
            empty["sources"]["folder_events"]["sha256"],
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn legacy_payload_is_not_exposed_and_envelope_is_not_synthesized() {
        let observation = project(
            b"title = 'ignored'",
            Some(b"{\"type\":\"user_message\",\"content\":\"SECRET\"}\n"),
        )
        .unwrap();
        assert_eq!(
            observation["last_event"],
            json!({"kind":"user_message", "event_id":null, "session_id":null, "recorded_at":null})
        );
        assert!(observation["recorded_native_session_id"].is_null());
        assert!(!observation.to_string().contains("SECRET"));
    }

    #[test]
    fn lifecycle_uses_file_order_not_timestamps_and_ignores_child() {
        let events = concat!(
            "{\"type\":\"session_lifecycle_updated\",\"state\":\"completed\",\"created_at\":\"2026-10-24T12:00:00Z\"}\n",
            "{\"type\":\"session_lifecycle_updated\",\"state\":\"running\",\"mode\":\"background\",\"created_at\":\"2020-01-01T00:00:00Z\"}\n",
            "{\"type\":\"child_session_status_changed\",\"state\":\"failed\"}\n"
        );
        let observation = project(MANIFEST, Some(events.as_bytes())).unwrap();
        assert_eq!(observation["event_count"], 3);
        assert_eq!(observation["last_lifecycle"]["state"], "running");
        assert_eq!(observation["last_lifecycle"]["mode"], "background");
        assert_eq!(
            observation["last_event"]["kind"],
            "child_session_status_changed"
        );
    }

    #[test]
    fn null_envelope_metadata_stays_null() {
        let observation = project(b"", Some(b"{\"type\":\"future_kind9\",\"event_id\":null,\"session_id\":null,\"created_at\":null}\n")).unwrap();
        assert_eq!(
            observation["last_event"],
            json!({"kind":"future_kind9", "event_id":null, "session_id":null, "recorded_at":null})
        );
    }

    #[test]
    fn duplicate_ids_and_keys_are_rejected_recursively() {
        for events in [
            "{\"type\":\"x\",\"event_id\":\"one\"}\n{\"type\":\"x\",\"event_id\":\"one\"}\n",
            "{\"type\":\"x\",\"type\":\"x\"}\n",
            "{\"type\":\"x\",\"payload\":{\"nested\":[{\"x\":1,\"x\":2}]}}\n",
        ] {
            assert!(
                project(MANIFEST, Some(events.as_bytes())).is_err(),
                "{events}"
            );
        }
    }

    #[test]
    fn schema_version_must_be_integer_one_when_present() {
        for version in ["null", "0", "2", "1.0", "\"1\"", "true", "{}"] {
            let events = format!("{{\"type\":\"x\",\"schema_version\":{version}}}\n");
            assert_eq!(
                project(MANIFEST, Some(events.as_bytes())).unwrap_err(),
                "unsupported_event_schema"
            );
        }
        assert!(project(MANIFEST, Some(b"{\"type\":\"x\",\"schema_version\":1}\n")).is_ok());
    }

    #[test]
    fn native_identity_is_checked_but_never_inferred() {
        let event =
            b"{\"type\":\"session_created\",\"id\":\"agt_other\",\"session_id\":\"agt_other\"}\n";
        assert_eq!(
            project(MANIFEST, Some(event)).unwrap_err(),
            "native_id_mismatch"
        );
        assert!(project(b"", Some(event)).unwrap()["recorded_native_session_id"].is_null());
        for events in [
            "{\"type\":\"x\",\"session_id\":\"agt_a\"}\n{\"type\":\"x\",\"session_id\":\"agt_b\"}\n",
            "{\"type\":\"session_created\",\"session_id\":\"agt_a\",\"id\":\"agt_b\"}\n",
            "{\"type\":\"x\",\"session_id\":\"ses_ui\"}\n",
            "{\"type\":\"session_created\",\"id\":\"agt_\"}\n",
            "{\"type\":\"session_created\",\"id\":null}\n",
        ] { assert!(project(b"", Some(events.as_bytes())).is_err()); }
    }

    #[test]
    fn invalid_metadata_types_and_empty_strings_are_rejected() {
        for key in ["event_id", "session_id", "created_at"] {
            for value in ["\"\"", "42", "false", "[]", "{}"] {
                let events = format!("{{\"type\":\"x\",\"{key}\":{value}}}\n");
                assert!(
                    project(MANIFEST, Some(events.as_bytes())).is_err(),
                    "{events}"
                );
            }
        }
        for manifest in [
            "session_id = ''",
            "session_id = 'ses_ui'",
            "session_id = 1",
            "created_at = ''",
            "created_at = 1",
            "created_at = '2026-10-24T12:00:00'",
            "created_at = 2026-10-24T12:00:00Z",
            "x = [",
            "session_id='agt_a'\nsession_id='agt_b'",
        ] {
            assert!(project(manifest.as_bytes(), None).is_err(), "{manifest}");
        }
    }

    #[test]
    fn timestamps_are_validated_and_preserved() {
        for time in [
            "",
            "today",
            "2026-10-24T12:00:00",
            "2026-99-24T12:00:00Z",
            &"x".repeat(65),
        ] {
            let events = format!("{{\"type\":\"x\",\"created_at\":{}}}\n", json!(time));
            assert!(project(MANIFEST, Some(events.as_bytes())).is_err());
        }
        let events = b"{\"type\":\"x\",\"created_at\":\"2026-10-24T12:00:00.123-04:00\"}\n";
        assert_eq!(
            project(MANIFEST, Some(events)).unwrap()["last_event"]["recorded_at"],
            "2026-10-24T12:00:00.123-04:00"
        );
    }

    #[test]
    fn kind_event_id_and_lifecycle_bounds() {
        for kind in ["", "Upper", "1x", "a-b", &"a".repeat(65)] {
            let events = format!("{{\"type\":{}}}\n", json!(kind));
            assert!(project(b"", Some(events.as_bytes())).is_err());
        }
        for id in ["", "a\nb", &"a".repeat(257)] {
            let events = format!("{{\"type\":\"x\",\"event_id\":{}}}\n", json!(id));
            assert!(project(b"", Some(events.as_bytes())).is_err());
        }
        let events = format!(
            "{{\"type\":\"{}\",\"event_id\":{}}}\n",
            "a".repeat(64),
            json!("a ".repeat(128))
        );
        assert!(project(b"", Some(events.as_bytes())).is_ok());
        for extra in [
            "",
            ",\"state\":null",
            ",\"state\":\"unknown\"",
            ",\"state\":\"running\",\"mode\":\"unknown\"",
            ",\"state\":\"running\",\"mode\":42",
        ] {
            let events = format!("{{\"type\":\"session_lifecycle_updated\"{extra}}}\n");
            assert!(project(b"", Some(events.as_bytes())).is_err());
        }
        assert!(project(
            b"",
            Some(b"{\"type\":\"session_lifecycle_updated\",\"state\":\"paused\",\"mode\":null}\n")
        )
        .is_ok());
    }

    #[test]
    fn incomplete_tail_and_malformed_interior_discard_evidence() {
        assert_eq!(
            project(b"", Some(b"{\"type\":\"x\"}")).unwrap_err(),
            "incomplete_tail"
        );
        for events in [
            "{bad}\n{\"type\":\"x\"}\n",
            "[]\n",
            "null\n",
            "{\"type\":\"x\",\"n\":NaN}\n",
            "{\"type\":\"x\",\"n\":1e999}\n",
        ] {
            assert!(project(b"", Some(events.as_bytes())).is_err());
        }
    }

    #[test]
    fn byte_line_and_record_caps() {
        assert!(project(&vec![b' '; MANIFEST_LIMIT], None).is_ok());
        assert_eq!(
            project(&vec![b' '; MANIFEST_LIMIT + 1], None).unwrap_err(),
            "manifest_limit"
        );
        assert!(project(b"", Some(&vec![b' '; EVENTS_LIMIT])).is_ok());
        assert_eq!(
            project(b"", Some(&vec![b' '; EVENTS_LIMIT + 1])).unwrap_err(),
            "events_limit"
        );
        let line = format!(
            "{{\"type\":\"x\",\"data\":\"{}\"}}\n",
            "x".repeat(LINE_LIMIT)
        );
        assert_eq!(
            project(b"", Some(line.as_bytes())).unwrap_err(),
            "line_limit"
        );
        let overhead = "{\"type\":\"x\",\"data\":\"\"}".len();
        let line = format!(
            "{{\"type\":\"x\",\"data\":\"{}\"}}\n",
            "x".repeat(LINE_LIMIT - overhead)
        );
        assert!(project(b"", Some(line.as_bytes())).is_ok());
        let record = "{\"type\":\"x\"}\n";
        assert!(project(b"", Some(record.repeat(RECORD_LIMIT).as_bytes())).is_ok());
        assert_eq!(
            project(b"", Some(record.repeat(RECORD_LIMIT + 1).as_bytes())).unwrap_err(),
            "record_limit"
        );
    }

    #[test]
    fn depth_and_container_caps_apply_inside_ignored_payloads() {
        let events = format!(
            "{{\"type\":\"x\",\"payload\":{}0{}}}\n",
            "[".repeat(31),
            "]".repeat(31)
        );
        assert!(project(b"", Some(events.as_bytes())).is_ok());
        let events = format!(
            "{{\"type\":\"x\",\"payload\":{}0{}}}\n",
            "[".repeat(32),
            "]".repeat(32)
        );
        assert!(project(b"", Some(events.as_bytes())).is_err());
        for payload in [
            format!("{}0{}", "[".repeat(33), "]".repeat(33)),
            format!("[{}0]", "0,".repeat(4096)),
            format!(
                "{{{}}}",
                (0..4097)
                    .map(|n| format!("\"{n}\":0"))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ] {
            let events = format!("{{\"type\":\"x\",\"payload\":{payload}}}\n");
            assert!(project(b"", Some(events.as_bytes())).is_err());
        }
        let events = format!("{{\"type\":\"x\",\"payload\":[{}0]}}\n", "0,".repeat(4095));
        assert!(project(b"", Some(events.as_bytes())).is_ok());
    }

    #[test]
    fn selectors_are_validated_before_io_without_echoing_input() {
        for path in [
            "relative",
            "ses_ui",
            "/tmp/../secret",
            "/tmp/a/",
            "/tmp/a\0b",
            "/tmp/a\nb",
            &format!("/{}", "x".repeat(4096)),
        ] {
            let error = observe(Path::new(path)).unwrap_err().to_string();
            assert_eq!(error, "observe requires an absolute UTF-8 folder path without parent components, controls, or trailing slash (maximum 4096 bytes)");
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_selector_is_rejected() {
        use std::os::unix::ffi::OsStrExt;
        assert!(observe(Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff"))).is_err());
    }

    #[test]
    fn native_identity_byte_limits() {
        assert!(native_id(&format!("agt_{}", "x".repeat(124))));
        for id in [
            format!("agt_{}", "x".repeat(125)),
            "agt_a.b".to_owned(),
            "agt_a b".to_owned(),
            "ses_ui".to_owned(),
            "agt_".to_owned(),
        ] {
            assert!(!native_id(&id));
        }
        assert!(native_id("agt_Az09_-"));
    }
}
