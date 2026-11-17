"""Verify a treeproof receipt with the reference Python implementations.

- DSSE signature: securesystemslib (Envelope.verify with an Ed25519 SSlibKey).
- Statement: in-toto-attestation's v1 Statement and ResourceDescriptor validators.
- Predicate: strict protobuf JSON parse into the test-result v0 message
  (unknown fields are rejected, so the predicate carries only spec fields).

Usage: python check_receipt.py RECEIPT.json PUBLIC_KEY.pem [EXPECTED_TREE]
Exits non-zero on any failure.
"""

import base64
import hashlib
import json
import sys

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
from google.protobuf import json_format
from in_toto_attestation.predicates.test_result.v0 import test_result_pb2
from in_toto_attestation.v1 import resource_descriptor, statement, statement_pb2
from securesystemslib.dsse import Envelope
from securesystemslib.signer import SSlibKey


def main() -> int:
    receipt_path, pem_path = sys.argv[1], sys.argv[2]
    expected_tree = sys.argv[3] if len(sys.argv) > 3 else None

    with open(receipt_path, "rb") as f:
        env_dict = json.loads(f.read())
    with open(pem_path, "rb") as f:
        pem = f.read()

    # Parse the PEM with OpenSSL (via cryptography), independently of treeproof.
    pub = serialization.load_pem_public_key(pem)
    assert isinstance(pub, Ed25519PublicKey), type(pub)
    public_hex = pub.public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw).hex()
    keyid = hashlib.sha256(bytes.fromhex(public_hex)).hexdigest()
    key = SSlibKey(keyid, "ed25519", "ed25519", {"public": public_hex})

    env = Envelope.from_dict(env_dict)
    assert env.payload_type == "application/vnd.in-toto+json", env.payload_type
    accepted = env.verify([key], 1)
    assert keyid in accepted, accepted
    print("dsse: signature verified by securesystemslib", keyid[:16])

    body = env.pae()  # recompute PAE through the reference implementation too
    assert body.startswith(b"DSSEv1 28 application/vnd.in-toto+json "), body[:60]

    payload = env.payload
    pb = json_format.Parse(payload, statement_pb2.Statement())
    stmt = statement.Statement.copy_from_pb(pb)
    stmt.validate()
    assert pb.predicate_type == "https://in-toto.io/attestation/test-result/v0.1", pb.predicate_type
    print("statement: valid in-toto v1 Statement")

    raw = json.loads(payload)
    tr = json_format.ParseDict(raw["predicate"], test_result_pb2.TestResult())
    assert tr.result in ("PASSED", "WARNED", "FAILED"), tr.result
    for rd in tr.configuration:
        resource_descriptor.ResourceDescriptor.copy_from_pb(rd).validate()
    print("predicate: strict test-result v0 parse OK, result", tr.result)

    tree = pb.subject[0].digest["gitTree"]
    if expected_tree is not None:
        assert tree == expected_tree, (tree, expected_tree)
    print("subject: gitTree", tree)

    # Tamper check: the reference verifier must reject a modified payload.
    forged = dict(env_dict)
    if b'"PASSED"' in payload:
        modified = payload.replace(b'"PASSED"', b'"FAILED"')
    else:
        modified = payload + b" "
    forged["payload"] = base64.b64encode(modified).decode()
    try:
        Envelope.from_dict(forged).verify([key], 1)
    except Exception:
        print("tamper: modified payload rejected")
    else:
        print("tamper: modified payload was ACCEPTED", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
