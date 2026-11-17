//! Property tests for the code paths that take untrusted input:
//! runner output, receipt JSON, envelopes, public keys, hook input and policy files.

use ed25519_dalek::SigningKey;
use proptest::prelude::*;
use treeproof::dsse::{self, PublicKey};
use treeproof::parse;
use treeproof::receipt;

fn key() -> (SigningKey, PublicKey) {
    let sk = SigningKey::from_bytes(&[42u8; 32]);
    let pk = PublicKey { key: sk.verifying_key() };
    (sk, pk)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn parsers_never_panic(s in "\\PC*") {
        let _ = parse::parse(&s, &[]);
    }

    #[test]
    fn parsers_never_panic_on_runner_like_lines(lines in prop::collection::vec(prop_oneof![
        "test result: (ok|FAILED)\\. [0-9]{1,25}; [0-9]{0,3} failed; .{0,20}",
        "test [a-z:]{0,20} \\.\\.\\. (ok|FAILED|ignored)",
        "=* ?[0-9]{1,25} (passed|failed|skipped|deselected)(, [0-9]{1,3} (passed|failed))? in [0-9.]{0,5}s ?=*",
        "Tests: +[0-9]{1,25} (passed|failed), [0-9]{1,3} total",
        " ?Tests +[0-9]{1,25} (passed|failed)( \\| [0-9]{1,3} skipped)? \\([0-9]{1,25}\\)",
        "--- (PASS|FAIL|SKIP): [A-Za-z]{0,10}",
        "(ok|FAIL|\\?) +[a-z./]{1,20}\t[0-9.]{1,5}s",
        "\\{\"Action\":\"(pass|fail|skip)\",\"Test\":\"[A-Za-z]{0,8}\"\\}",
        "\\s+Summary \\[ +[0-9.]{1,6}s\\] [0-9]{1,25} tests run: [0-9]{1,25} passed, [0-9]{1,3} skipped",
        "\u{1b}\\[[0-9;]{0,6}m?.{0,10}",
    ], 0..40)) {
        let text = lines.join("\n");
        let s = parse::parse(&text, &["go".to_string(), "test".to_string()]);
        // Saturating sums would be fine too, but counts must never exceed what the text could hold.
        prop_assert!(s.passed_names.len() <= parse::MAX_NAMES);
    }

    #[test]
    fn receipt_parsing_never_panics(s in "\\PC*") {
        let (_, pk) = key();
        prop_assert!(receipt::open_text(&s, &[pk]).is_err());
    }

    #[test]
    fn envelope_like_json_never_verifies_without_a_signature(payload in "[A-Za-z0-9+/=_-]{0,200}", sig in "[A-Za-z0-9+/=_-]{0,120}", ptype in "\\PC{0,40}") {
        let (_, pk) = key();
        let text = serde_json::json!({"payloadType": ptype, "payload": payload, "signatures": [{"keyid": "", "sig": sig}]}).to_string();
        prop_assert!(receipt::open_text(&text, &[pk]).is_err());
    }

    #[test]
    fn any_bit_flip_in_a_signed_payload_is_rejected(body in prop::collection::vec(any::<u8>(), 1..256), idx in any::<usize>(), bit in 0u8..8) {
        let (sk, pk) = key();
        let env = dsse::sign(&sk, dsse::IN_TOTO_PAYLOAD_TYPE, &body);
        prop_assert!(dsse::verify(&env, std::slice::from_ref(&pk), 1).is_ok());
        let mut tampered = body.clone();
        let i = idx % tampered.len();
        tampered[i] ^= 1 << bit;
        let mut bad = env.clone();
        use base64::Engine;
        bad.payload = base64::engine::general_purpose::STANDARD.encode(&tampered);
        prop_assert!(dsse::verify(&bad, &[pk], 1).is_err());
    }

    #[test]
    fn public_key_parsing_never_panics(s in "\\PC*") {
        let _ = PublicKey::parse(&s);
    }

    #[test]
    fn hook_input_never_panics(tool in "\\PC{0,12}", cmd in "\\PC{0,80}", out in "\\PC{0,200}") {
        // Only the pure helpers: the full handler touches git and the filesystem.
        let v = serde_json::json!({"tool_name": tool, "tool_input": {"command": cmd}, "tool_response": {"stdout": out}});
        let _ = treeproof::hooks::tool_output_text(&v["tool_response"]);
        let _ = treeproof::hooks::looks_like_test_command(&cmd);
        let _ = treeproof::hooks::detect_host(&v);
        let _ = treeproof::fileguard::patch_paths(&cmd, std::path::Path::new("/r"));
    }

    #[test]
    fn policy_parsing_never_panics(s in "\\PC{0,300}") {
        let _ = treeproof::config::Config::parse(&s);
    }

    #[test]
    fn collect_only_detection_never_panics(argv in prop::collection::vec("\\PC{0,12}", 0..8)) {
        let _ = parse::is_collect_only(&argv);
    }

    #[test]
    fn pae_length_prefixes_are_exact(t in "\\PC{0,50}", body in prop::collection::vec(any::<u8>(), 0..300)) {
        let p = dsse::pae(&t, &body);
        let expected_prefix = format!("DSSEv1 {} {} {} ", t.len(), t, body.len());
        prop_assert!(p.starts_with(expected_prefix.as_bytes()));
        prop_assert_eq!(p.len(), expected_prefix.len() + body.len());
    }
}
