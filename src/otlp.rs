//! OTLP/HTTP JSON exporter for `gen_ai.evaluation.result` events.
//!
//! Each verification check becomes one log record whose event name is
//! `gen_ai.evaluation.result` (OpenTelemetry GenAI semantic conventions), with
//! `gen_ai.evaluation.name` set to `verification.exit_code`,
//! `verification.test_rerun` or `verification.fs_diff` and
//! `gen_ai.evaluation.score.label` set to `verified`, `contradicted` or
//! `unknown` (the default). When `TRACEPARENT` holds a W3C trace context, the
//! records carry its trace and span id so they attach to the host's tool span.
//!
//! Transport is plain HTTP/1.1 with no TLS, meant for a local collector
//! (`http://localhost:4318`). For a remote HTTPS backend, run a collector next
//! to the agent and let it forward.

use crate::util::{now_ms, unhex};
use crate::{Error, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub const EVENT_NAME: &str = "gen_ai.evaluation.result";
pub const CHECK_EXIT_CODE: &str = "verification.exit_code";
pub const CHECK_TEST_RERUN: &str = "verification.test_rerun";
pub const CHECK_FS_DIFF: &str = "verification.fs_diff";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    Verified,
    Contradicted,
    Unknown,
}

impl Label {
    pub fn as_str(&self) -> &'static str {
        match self {
            Label::Verified => "verified",
            Label::Contradicted => "contradicted",
            Label::Unknown => "unknown",
        }
    }
    fn score(&self) -> Option<f64> {
        match self {
            Label::Verified => Some(1.0),
            Label::Contradicted => Some(0.0),
            Label::Unknown => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Evaluation {
    pub name: &'static str,
    pub label: Label,
    pub explanation: String,
}

/// W3C `traceparent`: returns (trace id hex, span id hex) when well formed.
pub fn parse_traceparent(tp: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = tp.trim().split('-').collect();
    if parts.len() < 4 || parts[0].len() != 2 || parts[1].len() != 32 || parts[2].len() != 16 {
        return None;
    }
    let t = parts[1].to_ascii_lowercase();
    let s = parts[2].to_ascii_lowercase();
    if unhex(&t).is_none() || unhex(&s).is_none() || t.bytes().all(|b| b == b'0') || s.bytes().all(|b| b == b'0') {
        return None;
    }
    Some((t, s))
}

fn attr_str(k: &str, v: &str) -> Value {
    json!({"key": k, "value": {"stringValue": v}})
}

/// Build an OTLP `ExportLogsServiceRequest` (JSON encoding).
pub fn build_request(evals: &[Evaluation], extra_attrs: &[(String, String)], traceparent: Option<&str>) -> Value {
    let ts = (now_ms() as u128 * 1_000_000).to_string();
    let link = traceparent.and_then(parse_traceparent);
    let records: Vec<Value> = evals
        .iter()
        .map(|e| {
            let mut attrs = vec![
                attr_str("gen_ai.evaluation.name", e.name),
                attr_str("gen_ai.evaluation.score.label", e.label.as_str()),
                attr_str("gen_ai.evaluation.explanation", &e.explanation),
            ];
            if let Some(v) = e.label.score() {
                attrs.push(json!({"key": "gen_ai.evaluation.score.value", "value": {"doubleValue": v}}));
            }
            for (k, v) in extra_attrs {
                attrs.push(attr_str(k, v));
            }
            let mut rec = json!({
                "timeUnixNano": ts,
                "observedTimeUnixNano": ts,
                "eventName": EVENT_NAME,
                "severityNumber": 9,
                "severityText": "INFO",
                "body": {"stringValue": format!("{}: {}", e.name, e.label.as_str())},
                "attributes": attrs,
            });
            if let Some((t, s)) = &link {
                rec["traceId"] = json!(t);
                rec["spanId"] = json!(s);
            }
            rec
        })
        .collect();
    json!({
        "resourceLogs": [{
            "resource": {"attributes": [attr_str("service.name", "treeproof")]},
            "scopeLogs": [{
                "scope": {"name": "treeproof", "version": env!("CARGO_PKG_VERSION")},
                "logRecords": records,
            }]
        }]
    })
}

/// Is automatic export allowed to use the generic `OTEL_EXPORTER_OTLP_*` endpoints?
/// Only when `TREEPROOF_OTEL=1` (or `on`/`true`): those variables are often set for other tools,
/// and the hooks should not start sending events just because they are.
pub fn generic_otel_opted_in() -> bool {
    std::env::var("TREEPROOF_OTEL").map(|v| v == "1" || v.eq_ignore_ascii_case("on") || v.eq_ignore_ascii_case("true")).unwrap_or(false)
}

/// The endpoint the hooks export to, if any. See [`endpoint_from_env_with`]; the generic
/// `OTEL_EXPORTER_OTLP_*` variables count only with `TREEPROOF_OTEL=1`.
pub fn endpoint_from_env() -> Option<String> {
    endpoint_from_env_with(generic_otel_opted_in())
}

/// Resolve the logs endpoint from `TREEPROOF_OTLP_ENDPOINT`, then (when `generic` is true)
/// `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` or `OTEL_EXPORTER_OTLP_ENDPOINT` (+ `/v1/logs`). Returns
/// `None` when export is not configured, `TREEPROOF_OTEL=0`, or the configured protocol is gRPC
/// (only HTTP/JSON is spoken). `treeproof otel` passes `generic = true` because the user asked
/// for an export.
pub fn endpoint_from_env_with(generic: bool) -> Option<String> {
    if std::env::var("TREEPROOF_OTEL").map(|v| v == "0" || v.eq_ignore_ascii_case("off")).unwrap_or(false) {
        return None;
    }
    if let Ok(e) = std::env::var("TREEPROOF_OTLP_ENDPOINT") {
        if !e.is_empty() {
            return Some(e);
        }
    }
    if !generic {
        return None;
    }
    let proto =
        std::env::var("OTEL_EXPORTER_OTLP_LOGS_PROTOCOL").or_else(|_| std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL")).unwrap_or_default();
    if proto.starts_with("grpc") {
        return None;
    }
    if let Ok(e) = std::env::var("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT") {
        if !e.is_empty() {
            return Some(e);
        }
    }
    if let Ok(e) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        if !e.is_empty() {
            return Some(format!("{}/v1/logs", e.trim_end_matches('/')));
        }
    }
    None
}

/// Extra headers from `OTEL_EXPORTER_OTLP_HEADERS` (`k=v,k2=v2`).
pub fn headers_from_env() -> Vec<(String, String)> {
    let raw = std::env::var("OTEL_EXPORTER_OTLP_LOGS_HEADERS").or_else(|_| std::env::var("OTEL_EXPORTER_OTLP_HEADERS")).unwrap_or_default();
    raw.split(',')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            let (k, v) = (k.trim(), v.trim());
            if k.is_empty() || k.contains(['\r', '\n']) || v.contains(['\r', '\n']) {
                return None;
            }
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

/// POST the request as OTLP/HTTP JSON. Returns the HTTP status code.
pub fn post(endpoint: &str, body: &Value, headers: &[(String, String)]) -> Result<u16> {
    let rest = endpoint
        .strip_prefix("http://")
        .ok_or_else(|| Error::new("only http:// OTLP endpoints are supported (use a local collector to forward to HTTPS)"))?;
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/v1/logs"),
    };
    let addr = if hostport.contains(':') && !hostport.ends_with(']') { hostport.to_string() } else { format!("{hostport}:80") };
    let payload = serde_json::to_vec(body)?;
    let mut req = format!(
        "POST {path} HTTP/1.1\r\nHost: {hostport}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nUser-Agent: treeproof/{}\r\n",
        payload.len(),
        env!("CARGO_PKG_VERSION")
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    let sock: std::net::SocketAddr = std::net::ToSocketAddrs::to_socket_addrs(&addr)
        .map_err(|e| Error::new(format!("resolve {addr}: {e}")))?
        .next()
        .ok_or_else(|| Error::new(format!("resolve {addr}: no address")))?;
    let mut s = TcpStream::connect_timeout(&sock, Duration::from_secs(3)).map_err(|e| Error::new(format!("connect {addr}: {e}")))?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.set_write_timeout(Some(Duration::from_secs(5))).ok();
    s.write_all(req.as_bytes()).and_then(|_| s.write_all(&payload)).map_err(|e| Error::new(format!("send: {e}")))?;
    let mut resp = Vec::new();
    let _ = s.take(64 * 1024).read_to_end(&mut resp);
    let head = String::from_utf8_lossy(&resp);
    let code = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or_else(|| Error::new("collector sent no HTTP status line"))?;
    Ok(code)
}

/// Export when configured; errors are returned, never panicked.
pub fn export_if_configured(evals: &[Evaluation], extra: &[(String, String)]) -> Option<Result<u16>> {
    let ep = endpoint_from_env()?;
    let tp = std::env::var("TRACEPARENT").ok();
    let body = build_request(evals, extra, tp.as_deref());
    Some(post(&ep, &body, &headers_from_env()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traceparent() {
        let (t, s) = parse_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").unwrap();
        assert_eq!(t, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(s, "00f067aa0ba902b7");
        assert!(parse_traceparent("00-00000000000000000000000000000000-00f067aa0ba902b7-01").is_none());
        assert!(parse_traceparent("garbage").is_none());
    }

    #[test]
    fn request_shape() {
        let evals = [
            Evaluation { name: CHECK_EXIT_CODE, label: Label::Verified, explanation: "x".into() },
            Evaluation { name: CHECK_TEST_RERUN, label: Label::Unknown, explanation: "y".into() },
        ];
        let v = build_request(
            &evals,
            &[("vcs.ref.head.revision".into(), "abc".into())],
            Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
        );
        let recs = &v["resourceLogs"][0]["scopeLogs"][0]["logRecords"];
        assert_eq!(recs.as_array().unwrap().len(), 2);
        assert_eq!(recs[0]["eventName"], EVENT_NAME);
        assert_eq!(recs[0]["traceId"], "4bf92f3577b34da6a3ce929d0e0e4736");
        let attrs = recs[0]["attributes"].as_array().unwrap();
        assert!(attrs.iter().any(|a| a["key"] == "gen_ai.evaluation.score.value" && a["value"]["doubleValue"] == 1.0));
        // Unknown carries no score value.
        let attrs = recs[1]["attributes"].as_array().unwrap();
        assert!(!attrs.iter().any(|a| a["key"] == "gen_ai.evaluation.score.value"));
    }

    #[test]
    fn post_to_local_listener() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = c.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                let s = String::from_utf8_lossy(&buf).to_string();
                if let Some(i) = s.find("\r\n\r\n") {
                    let len: usize =
                        s.lines().find_map(|l| l.strip_prefix("Content-Length: ")).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                    if buf.len() >= i + 4 + len {
                        break;
                    }
                }
            }
            c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap();
            String::from_utf8(buf).unwrap()
        });
        let body = build_request(&[Evaluation { name: CHECK_FS_DIFF, label: Label::Contradicted, explanation: "z".into() }], &[], None);
        let code = post(&format!("http://127.0.0.1:{port}/v1/logs"), &body, &[("x-a".into(), "b".into())]).unwrap();
        assert_eq!(code, 200);
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /v1/logs HTTP/1.1\r\n"));
        assert!(req.contains("x-a: b\r\n"));
        assert!(req.contains("\"eventName\":\"gen_ai.evaluation.result\""));
    }

    #[test]
    fn https_rejected() {
        assert!(post("https://example.com/v1/logs", &json!({}), &[]).is_err());
    }
}
