//! Receipts: in-toto Statement v1 with the test-result v0.1 predicate, signed in a DSSE envelope.
//!
//! The format is specified in `SPEC.md`; this module is the reference implementation.

use crate::dsse::{self, Envelope, PublicKey};
use crate::parse::Summary;
use crate::{Error, Result};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";
pub const PREDICATE_TYPE: &str = "https://in-toto.io/attestation/test-result/v0.1";
/// Name of the configuration descriptor that carries the run record.
pub const RUN_DESCRIPTOR: &str = "treeproof-run";
/// Name of the subject descriptor.
pub const SUBJECT_NAME: &str = "worktree";
/// Version of the run-record layout inside the annotations.
pub const RUN_SCHEMA: &str = "https://github.com/Mattbusel/treeproof/blob/main/SPEC.md#run-record-v1";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ResourceDescriptor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub digest: BTreeMap<String, String>,
    #[serde(default, rename = "downloadLocation", skip_serializing_if = "Option::is_none")]
    pub download_location: Option<String>,
    #[serde(default, rename = "mediaType", skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestResultPredicate {
    pub result: String,
    pub configuration: Vec<ResourceDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, rename = "passedTests")]
    pub passed_tests: Vec<String>,
    #[serde(default, rename = "warnedTests")]
    pub warned_tests: Vec<String>,
    #[serde(default, rename = "failedTests")]
    pub failed_tests: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Statement {
    #[serde(rename = "_type")]
    pub type_: String,
    pub subject: Vec<ResourceDescriptor>,
    #[serde(rename = "predicateType")]
    pub predicate_type: String,
    pub predicate: TestResultPredicate,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub passed: u64,
    pub failed: u64,
    pub skipped: u64,
    pub warned: u64,
}

/// How the run was obtained.
pub const MODE_FRESH: &str = "fresh";
/// Recorded from a host's own tool output by a PostToolUse hook.
pub const MODE_OBSERVED: &str = "observed";

/// The run record, stored as the annotations of the `treeproof-run` configuration descriptor.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInfo {
    pub schema: String,
    pub argv: Vec<String>,
    #[serde(default)]
    pub shell: bool,
    pub cwd: String,
    pub runners: Vec<String>,
    pub counts: Counts,
    pub exit_code: Option<i32>,
    pub mode: String,
    pub cache_key: String,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u64,
    #[serde(default)]
    pub head_commit: Option<String>,
    #[serde(default)]
    pub toolchains: BTreeMap<String, String>,
    /// Allowlisted environment variables: SHA-256 of the value, or null when unset.
    #[serde(default)]
    pub env: BTreeMap<String, Option<String>>,
    pub os: String,
    pub arch: String,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    pub treeproof_version: String,
}

/// A verified receipt.
#[derive(Clone, Debug)]
pub struct Receipt {
    pub envelope: Envelope,
    pub statement: Statement,
    pub run: RunInfo,
    /// The `gitTree` digest of the subject.
    pub tree: String,
    /// Key ids that signed it.
    pub keyids: Vec<String>,
}

impl Receipt {
    pub fn passed(&self) -> bool {
        self.statement.predicate.result == "PASSED"
    }
}

/// Assemble the statement for a run.
pub fn build_statement(
    tree: &str,
    result: &str,
    run: &RunInfo,
    lockfiles: &BTreeMap<String, String>,
    summary: &Summary,
) -> Result<Statement> {
    let mut subject_digest = BTreeMap::new();
    subject_digest.insert("gitTree".to_string(), tree.to_string());
    let annotations = match serde_json::to_value(run)? {
        serde_json::Value::Object(m) => m,
        _ => return Err(Error::new("run record did not serialise to an object")),
    };
    let mut run_digest = BTreeMap::new();
    run_digest.insert("sha256".to_string(), run.cache_key.clone());
    let mut configuration = vec![ResourceDescriptor {
        name: Some(RUN_DESCRIPTOR.to_string()),
        digest: run_digest,
        media_type: Some("application/vnd.treeproof.run+json".to_string()),
        annotations: Some(annotations),
        ..Default::default()
    }];
    for (path, sha) in lockfiles {
        let mut d = BTreeMap::new();
        d.insert("sha256".to_string(), sha.clone());
        configuration.push(ResourceDescriptor { name: Some(path.clone()), digest: d, ..Default::default() });
    }
    Ok(Statement {
        type_: STATEMENT_TYPE.to_string(),
        subject: vec![ResourceDescriptor { name: Some(SUBJECT_NAME.to_string()), digest: subject_digest, ..Default::default() }],
        predicate_type: PREDICATE_TYPE.to_string(),
        predicate: TestResultPredicate {
            result: result.to_string(),
            configuration,
            url: std::env::var("TREEPROOF_RUN_URL").ok().filter(|s| !s.is_empty()),
            passed_tests: summary.passed_names.clone(),
            warned_tests: summary.warned_names.clone(),
            failed_tests: summary.failed_names.clone(),
        },
    })
}

/// Serialise and sign a statement.
pub fn sign_statement(sk: &SigningKey, st: &Statement) -> Result<Envelope> {
    let body = serde_json::to_vec(st)?;
    Ok(dsse::sign(sk, dsse::IN_TOTO_PAYLOAD_TYPE, &body))
}

/// Parse envelope JSON (untrusted input).
pub fn parse_envelope(text: &str) -> Result<Envelope> {
    let env: Envelope = serde_json::from_str(text.trim()).map_err(|e| Error::new(format!("not a DSSE envelope: {e}")))?;
    if env.signatures.is_empty() {
        return Err(Error::new("envelope has no signatures"));
    }
    Ok(env)
}

/// Verify an envelope and parse the exact signed payload into a receipt.
pub fn open(env: &Envelope, trusted: &[PublicKey]) -> Result<Receipt> {
    let v = dsse::verify(env, trusted, 1)?;
    let statement: Statement =
        serde_json::from_slice(&v.payload).map_err(|e| Error::new(format!("signed payload is not a test-result statement: {e}")))?;
    if statement.type_ != STATEMENT_TYPE {
        return Err(Error::new(format!("unexpected statement _type {:?}", statement.type_)));
    }
    if statement.predicate_type != PREDICATE_TYPE {
        return Err(Error::new(format!("unexpected predicateType {:?}", statement.predicate_type)));
    }
    if !matches!(statement.predicate.result.as_str(), "PASSED" | "WARNED" | "FAILED") {
        return Err(Error::new(format!("invalid result {:?}", statement.predicate.result)));
    }
    let tree = statement
        .subject
        .iter()
        .find_map(|s| s.digest.get("gitTree").cloned())
        .ok_or_else(|| Error::new("statement subject has no gitTree digest"))?;
    if !crate::util::is_git_oid(&tree) {
        return Err(Error::new("subject gitTree is not a git object id"));
    }
    let desc = statement
        .predicate
        .configuration
        .iter()
        .find(|d| d.name.as_deref() == Some(RUN_DESCRIPTOR))
        .ok_or_else(|| Error::new("statement has no treeproof-run configuration descriptor"))?;
    let ann = desc.annotations.clone().unwrap_or_default();
    let run: RunInfo = serde_json::from_value(serde_json::Value::Object(ann)).map_err(|e| Error::new(format!("bad run record: {e}")))?;
    if run.schema != RUN_SCHEMA {
        return Err(Error::new(format!("unknown run record schema {:?}", run.schema)));
    }
    if desc.digest.get("sha256") != Some(&run.cache_key) {
        return Err(Error::new("run descriptor digest does not match the run record's cache key"));
    }
    Ok(Receipt { envelope: env.clone(), statement, run, tree, keyids: v.keyids })
}

/// Parse and verify receipt text in one step.
pub fn open_text(text: &str, trusted: &[PublicKey]) -> Result<Receipt> {
    open(&parse_envelope(text)?, trusted)
}
