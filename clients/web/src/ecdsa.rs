//! ECDSA P-256 signatures, in the two shapes the browser and the host disagree about.
//!
//! WebCrypto's `ECDSA` produces and consumes raw `r || s`, 32 bytes each. Everything on the host
//! side — `aws-lc-rs`, rcgen, X.509 — speaks ASN.1 DER. Neither will take the other's, so one
//! side has to convert, and doing it here rather than in JavaScript keeps it in one place with
//! tests around it.
//!
//! Only P-256 is handled, because the whole browser credential path is P-256: the host's verifier
//! rejects anything else, so a key that got this far already is one.

/// Wrap a raw `r || s` signature as `SEQUENCE { INTEGER r, INTEGER s }`.
///
/// DER integers are signed and minimal, so a value with a high top bit gains a leading zero and
/// leading zero bytes are dropped. Getting that wrong yields a signature that parses on some
/// verifiers and not others, which is the worst possible failure.
pub fn raw_to_der(raw: &[u8; 64]) -> Vec<u8> {
    let (r, s) = raw.split_at(32);
    let (r, s) = (der_int(r), der_int(s));
    let body = r.len() + s.len();
    let mut out = Vec::with_capacity(body + 2);
    out.push(0x30);
    out.push(body as u8); // A P-256 pair is at most 72 bytes: never the long form.
    out.extend_from_slice(&r);
    out.extend_from_slice(&s);
    out
}

/// One `INTEGER`, minimally encoded and never negative.
fn der_int(v: &[u8]) -> Vec<u8> {
    let v = &v[v.iter().take_while(|b| **b == 0).count()..];
    let mut out = Vec::with_capacity(v.len() + 3);
    out.push(0x02);
    if v.is_empty() {
        out.push(1);
        out.push(0);
        return out;
    }
    out.push((v.len() + usize::from(v[0] & 0x80 != 0)) as u8);
    if v[0] & 0x80 != 0 {
        out.push(0);
    }
    out.extend_from_slice(v);
    out
}

/// Unwrap a DER signature to the raw `r || s` WebCrypto wants, or `None` if it is not one.
///
/// The browser never needs this — it only ever signs — but a round trip is what actually proves
/// the encoder above, so it stays for the tests.
///
/// Strict: a trailing byte, a long-form length or an over-long integer is a refusal, not
/// something to read past. This parses input from the network.
#[cfg_attr(not(test), allow(dead_code))]
pub fn der_to_raw(der: &[u8]) -> Option<[u8; 64]> {
    let body = der.strip_prefix(&[0x30])?;
    let (len, body) = body.split_first()?;
    if usize::from(*len) != body.len() || *len & 0x80 != 0 {
        return None;
    }
    let (r, body) = take_int(body)?;
    let (s, rest) = take_int(body)?;
    if !rest.is_empty() {
        return None;
    }
    let mut out = [0u8; 64];
    out[32 - r.len()..32].copy_from_slice(r);
    out[64 - s.len()..].copy_from_slice(s);
    Some(out)
}

/// One `INTEGER`, returned without its sign padding, and only if it fits a P-256 coordinate.
#[cfg_attr(not(test), allow(dead_code))]
fn take_int(b: &[u8]) -> Option<(&[u8], &[u8])> {
    let b = b.strip_prefix(&[0x02])?;
    let (len, b) = b.split_first()?;
    let (v, rest) = b.split_at_checked(usize::from(*len))?;
    let v = &v[v.iter().take_while(|x| **x == 0).count()..];
    (v.len() <= 32).then_some((v, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every coordinate shape the two encodings differ on: a high top bit needs a pad, leading
    /// zeros must not survive, and zero itself is one byte.
    #[test]
    fn the_two_shapes_round_trip() {
        let cases: [[u8; 64]; 5] = [
            [0x11; 64],
            [0xff; 64], // both halves need a sign pad
            std::array::from_fn(|i| if i < 32 { 0 } else { 0x7f }), // r is zero
            std::array::from_fn(|i| u8::try_from(i).unwrap()), // r starts 0x00, s does not
            [0; 64],
        ];
        for raw in cases {
            let der = raw_to_der(&raw);
            assert_eq!(der[0], 0x30, "SEQUENCE");
            assert_eq!(usize::from(der[1]), der.len() - 2, "length covers the body");
            assert_eq!(der_to_raw(&der), Some(raw), "{raw:?}");
        }
    }

    /// A signature arrives over the network, so a malformed one has to be refused rather than
    /// read past.
    #[test]
    fn malformed_der_is_refused() {
        let good = raw_to_der(&[0x11; 64]);
        assert!(der_to_raw(&[]).is_none());
        assert!(der_to_raw(&good[..good.len() - 1]).is_none(), "truncated");
        let mut long = good.clone();
        long.push(0);
        assert!(der_to_raw(&long).is_none(), "trailing byte");
        let mut wrong_tag = good.clone();
        wrong_tag[0] = 0x31;
        assert!(der_to_raw(&wrong_tag).is_none());
        // 33 payload bytes is longer than a P-256 coordinate: a different curve, or a lie.
        let mut wide = vec![0x30, 0x46, 0x02, 0x21];
        wide.extend_from_slice(&[0x7f; 33]);
        wide.extend_from_slice(&[0x02, 0x21]);
        wide.extend_from_slice(&[0x7f; 33]);
        assert!(der_to_raw(&wide).is_none());
    }
}
