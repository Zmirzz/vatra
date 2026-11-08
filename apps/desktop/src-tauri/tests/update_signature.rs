//! Offline signature-primitive regression test for the verifier used by
//! tauri-plugin-updater 2.14.1 (`minisign-verify 0.3`, verify(..., true)).
//! This is not a live update-endpoint or installer test.
//!
//! Public test vector from minisign-verify 0.3.0 src/lib.rs (MIT):
//! https://github.com/jedisct1/rust-minisign-verify
//! Copyright (c) 2019-2025 Frank Denis.
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//! The above copyright notice and this permission notice shall be included in
//! all copies or substantial portions of the Software.
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
//! THE SOFTWARE.

use minisign_verify::{PublicKey, Signature};

// These are public verification data only; no private key is read or needed.
const PUBLIC_KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
const SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1633700835\tfile:test\tprehashed\nwLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==";

#[test]
fn update_signature_checked_and_tampered_payload_rejected() {
    let key = PublicKey::from_base64(PUBLIC_KEY).expect("public test key decodes");
    let signature = Signature::decode(SIGNATURE).expect("public test signature decodes");

    key.verify(b"test", &signature, true)
        .expect("the original artifact must pass the updater's signature primitive");
    assert!(key.verify(b"tesT", &signature, true).is_err(), "one modified artifact byte must be rejected");
}

#[test]
fn update_signature_and_authenticated_comment_tampering_rejected() {
    let key = PublicKey::from_base64(PUBLIC_KEY).unwrap();
    // Keep valid encoding and key ID, but alter signature bytes.
    let altered = SIGNATURE.replace("559r3g7", "559r3g8");
    let signature = Signature::decode(&altered).expect("tampered signature still has valid encoding");
    assert!(key.verify(b"test", &signature, true).is_err());

    // The third argument true is also used by Tauri: verify the global signature
    // over the trusted comment, rather than accepting attacker-changed metadata.
    let altered = SIGNATURE.replace("file:test", "file:other");
    let signature = Signature::decode(&altered).unwrap();
    assert!(key.verify(b"test", &signature, true).is_err());
}
