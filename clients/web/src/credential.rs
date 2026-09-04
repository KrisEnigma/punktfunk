//! The browser's device credential: pairing with a WebCrypto key, and proving it afterwards.
//!
//! A browser presents no client certificate, so the identity mTLS gave every other client has to
//! come from somewhere else. The page generates a **non-extractable** P-256 keypair and keeps it
//! in IndexedDB; this module never sees the private half — it holds the SPKI, whose SHA-256 is
//! the fingerprint the host stores, and asks the page to sign when a signature is needed.
//!
//! Both flows live here because both are the same key. Pairing is SPAKE2 role A against the PIN,
//! the identity being that fingerprint and the host's being the certificate hash the page had to
//! pin to connect. Afterwards each session opens with a host nonce the page signs, bound to that
//! same hash — see `design/web-client-implementation-plan.md` Phase 3.

use punktfunk_core::quic::{
    auth_signed_message, pake, AuthResponse, PairChallenge, PairProof, PairRequest, PairResult,
};
use std::cell::RefCell;

/// Where the credential has got to. The page polls this; a browser cannot block, and signing is
/// asynchronous even when nothing is waiting on the network.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum Cred {
    /// No device key handed over yet.
    #[default]
    Empty,
    /// A key is loaded. Nothing in flight.
    Ready,
    /// The host asked for a signature and [`signing_message`] is what to sign.
    NeedsSignature,
    /// Pairing: `PairRequest` sent, waiting for the host's challenge.
    Pairing,
    /// Pairing finished and the host said yes. The page stores the host fingerprint now.
    Paired,
    /// Pairing was refused, or a message did not parse. Terminal for this connection.
    Failed,
}

#[derive(Default)]
struct Credential {
    /// SPKI DER of the device key. Empty until the page hands it over.
    spki: Vec<u8>,
    /// SHA-256 of `spki` — what the host stores and looks a session up by.
    fingerprint: [u8; 32],
    /// SHA-256 of the transport certificate this connection is on: the SPAKE2 host identity
    /// while pairing, and the channel binding afterwards.
    host_fp: [u8; 32],
    /// The exact bytes the page must sign, valid only while [`Cred::NeedsSignature`].
    to_sign: Vec<u8>,
    /// SPAKE2 role A, live between `PairRequest` and `PairProof`.
    pake: Option<pake::PairingPake>,
    phase: Cred,
}

thread_local! {
    static CRED: RefCell<Credential> = RefCell::new(Credential::default());
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes).into()
}

/// Take the page's device key and the certificate hash of the connection it is about to open.
///
/// `spki` is the public half only — the page keeps the private key non-extractable, which is the
/// point: a script that reads IndexedDB gets something that cannot sign.
///
/// # Safety
/// `spki` must point to `spki_len` readable bytes and `host_fp` to 32, for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pf_device_set(spki: *const u8, spki_len: u32, host_fp: *const u8) -> i32 {
    if spki.is_null() || host_fp.is_null() || spki_len == 0 {
        return 0;
    }
    // SAFETY: the caller guarantees both ranges for the call; both are copied out here.
    let (spki, hfp) = unsafe {
        (
            std::slice::from_raw_parts(spki, spki_len as usize).to_vec(),
            std::slice::from_raw_parts(host_fp, 32),
        )
    };
    CRED.with(|c| {
        let mut c = c.borrow_mut();
        c.fingerprint = sha256(&spki);
        c.spki = spki;
        c.host_fp.copy_from_slice(hfp);
        c.phase = Cred::Ready;
        1
    })
}

/// The device fingerprint as 64 lowercase hex characters — what the console shows and what the
/// host's paired-device list holds.
///
/// # Safety
/// `out` must point to 64 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pf_device_fingerprint_hex(out: *mut u8) -> i32 {
    if out.is_null() {
        return 0;
    }
    CRED.with(|c| {
        let c = c.borrow();
        if c.phase == Cred::Empty {
            return 0;
        }
        let hex: Vec<u8> = c
            .fingerprint
            .iter()
            .flat_map(|b| format!("{b:02x}").into_bytes())
            .collect();
        // SAFETY: `hex` is exactly 64 bytes and the caller promises 64 writable.
        unsafe { std::ptr::copy_nonoverlapping(hex.as_ptr(), out, 64) };
        1
    })
}

pub fn phase() -> Cred {
    CRED.with(|c| c.borrow().phase)
}

/// `0` empty, `1` ready, `2` needs a signature, `3` pairing, `4` paired, `5` failed.
#[unsafe(no_mangle)]
pub extern "C" fn pf_cred_phase() -> u32 {
    match phase() {
        Cred::Empty => 0,
        Cred::Ready => 1,
        Cred::NeedsSignature => 2,
        Cred::Pairing => 3,
        Cred::Paired => 4,
        Cred::Failed => 5,
    }
}

/// The bytes the page must sign while [`Cred::NeedsSignature`]. Empty otherwise.
///
/// The page uses the pointer pair below instead; this is what lets a test say which bytes those
/// are, and both sides signing the same message is the one thing that must not drift.
#[cfg_attr(not(test), allow(dead_code))]
pub fn signing_message() -> Vec<u8> {
    CRED.with(|c| c.borrow().to_sign.clone())
}

/// Where those bytes live, so the page signs them without a copy through JavaScript.
///
/// Valid until the next call into this module — read it, sign it, and do not hold it. The
/// pointer is null and the length zero unless a signature is actually being asked for.
#[unsafe(no_mangle)]
pub extern "C" fn pf_cred_sign_ptr() -> *const u8 {
    CRED.with(|c| {
        let c = c.borrow();
        if c.phase == Cred::NeedsSignature {
            c.to_sign.as_ptr()
        } else {
            std::ptr::null()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pf_cred_sign_len() -> u32 {
    CRED.with(|c| {
        let c = c.borrow();
        if c.phase == Cred::NeedsSignature {
            c.to_sign.len() as u32
        } else {
            0
        }
    })
}

/// The host's session nonce arrived. Nothing is sent yet: only the page can sign.
///
/// A browser with no key ignores this — an unpaired one still connects, and a host that requires
/// pairing is the side that refuses it.
pub fn on_challenge(nonce: &[u8; 32]) {
    CRED.with(|c| {
        let mut c = c.borrow_mut();
        if c.phase != Cred::Ready {
            return;
        }
        c.to_sign = auth_signed_message(&c.host_fp, nonce);
        c.phase = Cred::NeedsSignature;
    });
}

/// The page's raw `r || s` signature over [`signing_message`], as an encoded `AuthResponse`.
///
/// Returns `None` when nothing was waiting on a signature, so a page that calls this twice
/// cannot put a second credential on the wire.
pub fn auth_response(raw_sig: &[u8; 64]) -> Option<Vec<u8>> {
    CRED.with(|c| {
        let mut c = c.borrow_mut();
        if c.phase != Cred::NeedsSignature {
            return None;
        }
        c.to_sign.clear();
        c.phase = Cred::Ready;
        Some(
            AuthResponse {
                device_key: c.spki.clone(),
                signature: crate::ecdsa::raw_to_der(raw_sig),
            }
            .encode(),
        )
    })
}

/// Start pairing: SPAKE2 role A over the PIN, bound to this device key and this connection's
/// certificate. Returns the `PairRequest` to send, or `None` without a key.
pub fn pair_begin(pin: &str, name: &str) -> Option<Vec<u8>> {
    CRED.with(|c| {
        let mut c = c.borrow_mut();
        if c.spki.is_empty() {
            return None;
        }
        let (state, spake_a) = pake::start(true, pin, &c.fingerprint, &c.host_fp);
        c.pake = Some(state);
        c.phase = Cred::Pairing;
        Some(
            PairRequest {
                name: name.to_string(),
                spake_a,
                device_key: c.spki.clone(),
            }
            .encode(),
        )
    })
}

/// The host's SPAKE2 message and confirmation. Returns the `PairProof` to send back.
///
/// The host's MAC is checked first and a mismatch ends this here: it means a wrong PIN or a man
/// in the middle, and either way we must not prove our own key to whoever is on the other end.
pub fn on_pair_challenge(ch: &PairChallenge) -> Option<Vec<u8>> {
    CRED.with(|c| {
        let mut c = c.borrow_mut();
        let pake = c.pake.take()?;
        let confirms = match pake.finish(&ch.spake_b) {
            Ok(c) => c,
            Err(_) => {
                c.phase = Cred::Failed;
                return None;
            }
        };
        if !pake::verify(&confirms.host, &ch.confirm) {
            c.phase = Cred::Failed;
            return None;
        }
        Some(
            PairProof {
                confirm: confirms.client,
            }
            .encode(),
        )
    })
}

/// The host's verdict. The page stores the host fingerprint only on `Paired`.
pub fn on_pair_result(r: &PairResult) {
    CRED.with(|c| {
        let mut c = c.borrow_mut();
        c.phase = if r.ok { Cred::Paired } else { Cred::Failed };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(spki: &[u8], host_fp: [u8; 32]) {
        CRED.with(|c| {
            let mut c = c.borrow_mut();
            c.fingerprint = sha256(spki);
            c.spki = spki.to_vec();
            c.host_fp = host_fp;
            c.pake = None;
            c.to_sign.clear();
            c.phase = Cred::Ready;
        });
    }

    /// The whole ceremony against the host's own role B, so a divergence in either half shows up
    /// here rather than against a live host.
    #[test]
    fn pairing_agrees_with_the_host_half() {
        let spki = b"a device key".as_slice();
        let host_fp = [0x42u8; 32];
        load(spki, host_fp);

        let req = PairRequest::decode(&pair_begin("1234", "Browser").unwrap()).unwrap();
        assert_eq!(
            req.device_key, spki,
            "the host derives our identity from this"
        );
        assert_eq!(phase(), Cred::Pairing);

        // The host's side: same PIN, and it takes our identity from the key we just sent.
        let client_fp = sha256(&req.device_key);
        let (host, spake_b) = pake::start(false, "1234", &client_fp, &host_fp);
        let hc = host.finish(&req.spake_a).unwrap();
        let proof = PairProof::decode(
            &on_pair_challenge(&PairChallenge {
                spake_b,
                confirm: hc.host,
            })
            .unwrap(),
        )
        .unwrap();
        assert!(pake::verify(&hc.client, &proof.confirm), "host accepts us");
        on_pair_result(&PairResult { ok: true });
        assert_eq!(phase(), Cred::Paired);
    }

    /// A wrong PIN must not get our proof: the host's MAC is checked first, and failing it ends
    /// the ceremony rather than answering.
    #[test]
    fn a_bad_host_confirmation_is_never_answered() {
        load(b"a device key", [0x42; 32]);
        let req = PairRequest::decode(&pair_begin("1234", "Browser").unwrap()).unwrap();
        let (host, spake_b) = pake::start(false, "9999", &sha256(&req.device_key), &[0x42; 32]);
        let hc = host.finish(&req.spake_a).unwrap();
        assert!(
            on_pair_challenge(&PairChallenge {
                spake_b,
                confirm: hc.host,
            })
            .is_none(),
            "wrong PIN: no proof goes back"
        );
        assert_eq!(phase(), Cred::Failed);
    }

    /// The session credential is offered once. A second call must not put another on the wire.
    #[test]
    fn a_signature_is_spent_when_it_is_used() {
        let spki = b"a device key".as_slice();
        load(spki, [0x42; 32]);
        assert!(auth_response(&[7; 64]).is_none(), "nothing asked for one");

        on_challenge(&[0x99; 32]);
        assert_eq!(phase(), Cred::NeedsSignature);
        assert_eq!(
            signing_message(),
            auth_signed_message(&[0x42; 32], &[0x99; 32]),
            "both sides must sign the same bytes"
        );

        let sent = AuthResponse::decode(&auth_response(&[7; 64]).unwrap()).unwrap();
        assert_eq!(sent.device_key, spki);
        assert_eq!(
            crate::ecdsa::der_to_raw(&sent.signature),
            Some([7u8; 64]),
            "the host reads DER, WebCrypto writes raw"
        );
        assert!(signing_message().is_empty());
        assert!(auth_response(&[7; 64]).is_none(), "spent");
    }
}
