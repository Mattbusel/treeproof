//! DSSE (Dead Simple Signing Envelope) v1 with Ed25519 keys.
//!
//! Signing covers `PAE(payloadType, payload)`. Verification decodes the
//! payload once, checks the signature over those exact bytes, and returns those
//! same bytes to the caller, as the DSSE protocol requires.

use crate::util::{hex, sha256_hex, unhex};
use crate::{Error, Result};
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// The in-toto payload type.
pub const IN_TOTO_PAYLOAD_TYPE: &str = "application/vnd.in-toto+json";

/// Ed25519 SubjectPublicKeyInfo DER prefix (RFC 8410); the 32 key bytes follow.
const SPKI_PREFIX: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvSignature {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub keyid: String,
    pub sig: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    #[serde(rename = "payloadType")]
    pub payload_type: String,
    pub payload: String,
    pub signatures: Vec<EnvSignature>,
}

/// Pre-Authentication Encoding from the DSSE protocol.
pub fn pae(payload_type: &str, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + payload_type.len() + 32);
    out.extend_from_slice(b"DSSEv1 ");
    out.extend_from_slice(payload_type.len().to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload_type.as_bytes());
    out.push(b' ');
    out.extend_from_slice(body.len().to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(body);
    out
}

/// Decode base64 in any of the four alphabets DSSE allows (standard or URL-safe, padded or not).
pub fn b64_any(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    STANDARD.decode(s).or_else(|_| URL_SAFE.decode(s)).or_else(|_| STANDARD_NO_PAD.decode(s)).or_else(|_| URL_SAFE_NO_PAD.decode(s)).ok()
}

/// An Ed25519 public key with its treeproof key id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKey {
    pub key: VerifyingKey,
}

impl PublicKey {
    pub fn from_bytes(b: &[u8]) -> Result<PublicKey> {
        let arr: [u8; 32] = b.try_into().map_err(|_| Error::new("an Ed25519 public key is 32 bytes"))?;
        let key = VerifyingKey::from_bytes(&arr).map_err(|e| Error::new(format!("invalid Ed25519 public key: {e}")))?;
        Ok(PublicKey { key })
    }

    /// Key id: lowercase hex SHA-256 of the 32 raw public key bytes.
    pub fn keyid(&self) -> String {
        sha256_hex(self.key.as_bytes())
    }

    pub fn raw_hex(&self) -> String {
        hex(self.key.as_bytes())
    }

    /// PEM `PUBLIC KEY` (SubjectPublicKeyInfo), the format cosign and OpenSSL use.
    pub fn to_pem(&self) -> String {
        let mut der = SPKI_PREFIX.to_vec();
        der.extend_from_slice(self.key.as_bytes());
        let b = STANDARD.encode(der);
        format!("-----BEGIN PUBLIC KEY-----\n{b}\n-----END PUBLIC KEY-----\n")
    }

    /// Parse PEM SPKI, 64-char hex, or base64 of the raw 32 bytes.
    pub fn parse(text: &str) -> Result<PublicKey> {
        let t = text.trim();
        if t.contains("-----BEGIN PUBLIC KEY-----") {
            let body: String = t.lines().filter(|l| !l.starts_with("-----")).map(|l| l.trim()).collect();
            let der = b64_any(&body).ok_or_else(|| Error::new("PEM body is not base64"))?;
            if der.len() != 44 || der[..12] != SPKI_PREFIX {
                return Err(Error::new("PEM is not an Ed25519 SubjectPublicKeyInfo"));
            }
            return PublicKey::from_bytes(&der[12..]);
        }
        if t.len() == 64 {
            if let Some(b) = unhex(t) {
                return PublicKey::from_bytes(&b);
            }
        }
        if let Some(b) = b64_any(t) {
            if b.len() == 32 {
                return PublicKey::from_bytes(&b);
            }
        }
        Err(Error::new("unrecognised public key format (expected PEM, hex or base64 Ed25519)"))
    }
}

/// Sign `body` as `payload_type` and wrap it in an envelope.
pub fn sign(sk: &SigningKey, payload_type: &str, body: &[u8]) -> Envelope {
    let sig = sk.sign(&pae(payload_type, body));
    let pk = PublicKey { key: sk.verifying_key() };
    Envelope {
        payload_type: payload_type.to_string(),
        payload: STANDARD.encode(body),
        signatures: vec![EnvSignature { keyid: pk.keyid(), sig: STANDARD.encode(sig.to_bytes()) }],
    }
}

/// Outcome of a successful verification.
#[derive(Clone, Debug)]
pub struct Verified {
    /// The exact payload bytes the signature covers.
    pub payload: Vec<u8>,
    /// Key ids of the trusted keys that produced a valid signature.
    pub keyids: Vec<String>,
}

/// Verify an envelope against trusted keys. At least `threshold` distinct
/// trusted keys must have produced a valid signature. Key ids in the envelope are
/// only hints: every trusted key is tried against every signature.
pub fn verify(env: &Envelope, trusted: &[PublicKey], threshold: usize) -> Result<Verified> {
    if env.payload_type != IN_TOTO_PAYLOAD_TYPE {
        return Err(Error::new(format!("unsupported payloadType {:?}", env.payload_type)));
    }
    let payload = b64_any(&env.payload).ok_or_else(|| Error::new("payload is not valid base64"))?;
    let msg = pae(&env.payload_type, &payload);
    let mut accepted: Vec<String> = Vec::new();
    for s in &env.signatures {
        let Some(raw) = b64_any(&s.sig) else { continue };
        let Ok(sig) = Signature::from_slice(&raw) else { continue };
        for k in trusted {
            let id = k.keyid();
            if accepted.contains(&id) {
                continue;
            }
            if k.key.verify(&msg, &sig).is_ok() {
                accepted.push(id);
            }
        }
        if accepted.len() >= threshold.max(1) {
            break;
        }
    }
    if accepted.len() < threshold.max(1) {
        return Err(Error::new(if trusted.is_empty() {
            "no trusted keys to verify against".to_string()
        } else {
            "signature does not verify against any trusted key".to_string()
        }));
    }
    Ok(Verified { payload, keyids: accepted })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pae_matches_dsse_test_vector() {
        // From the DSSE protocol document.
        let p = pae("http://example.com/HelloWorld", b"hello world");
        assert_eq!(p, b"DSSEv1 29 http://example.com/HelloWorld 11 hello world");
    }

    #[test]
    fn pae_empty() {
        assert_eq!(pae("", b""), b"DSSEv1 0  0 ");
    }

    #[test]
    fn rfc8032_test_vector_1() {
        // RFC 8032 section 7.1, TEST 1 (empty message).
        let sk =
            SigningKey::from_bytes(&unhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60").unwrap().try_into().unwrap());
        assert_eq!(hex(sk.verifying_key().as_bytes()), "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        let sig = sk.sign(b"");
        assert_eq!(
            hex(&sig.to_bytes()),
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
        );
    }

    #[test]
    fn sign_verify_and_tamper() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let pk = PublicKey { key: sk.verifying_key() };
        let env = sign(&sk, IN_TOTO_PAYLOAD_TYPE, b"{\"a\":1}");
        let v = verify(&env, std::slice::from_ref(&pk), 1).unwrap();
        assert_eq!(v.payload, b"{\"a\":1}");
        assert_eq!(v.keyids, vec![pk.keyid()]);

        let mut bad = env.clone();
        bad.payload = STANDARD.encode(b"{\"a\":2}");
        assert!(verify(&bad, std::slice::from_ref(&pk), 1).is_err());

        let other = PublicKey { key: SigningKey::from_bytes(&[8u8; 32]).verifying_key() };
        assert!(verify(&env, &[other], 1).is_err());
        assert!(verify(&env, &[], 1).is_err());

        let mut wrong_type = env.clone();
        wrong_type.payload_type = "application/json".into();
        assert!(verify(&wrong_type, std::slice::from_ref(&pk), 1).is_err());

        // URL-safe, unpadded encodings must verify too.
        let mut urlsafe = env.clone();
        urlsafe.payload = URL_SAFE_NO_PAD.encode(b"{\"a\":1}");
        urlsafe.signatures[0].sig = URL_SAFE_NO_PAD.encode(b64_any(&env.signatures[0].sig).unwrap());
        assert!(verify(&urlsafe, &[pk], 1).is_ok());
    }

    #[test]
    fn pem_roundtrip() {
        let pk = PublicKey { key: SigningKey::from_bytes(&[9u8; 32]).verifying_key() };
        let pem = pk.to_pem();
        assert_eq!(PublicKey::parse(&pem).unwrap(), pk);
        assert_eq!(PublicKey::parse(&pk.raw_hex()).unwrap(), pk);
        assert!(PublicKey::parse("nonsense").is_err());
    }
}
