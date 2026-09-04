// Getting to a host: what to pin, and whether to trust it.
//
// A browser must know the WebTransport certificate hash before it can dial, and
// `GET /api/v1/webtransport` is the only way to learn it. That route is unauthenticated and
// cannot be otherwise — a browser that has never paired holds no credential — so a first
// connection is trust-on-first-use, exactly as the native clients' is, and PAKE pairing is what
// actually proves the host.
//
// Afterwards it is not TOFU any more. Pairing stores the host's long-lived fingerprint, and the
// route publishes that identity's signature over the short-lived hash; `verify` below is a
// paired browser refusing to dial a host that cannot produce it. See
// `design/web-client-implementation-plan.md` Phase 3.

const CTX = "punktfunk-wt-cert-v1:";

const hex = (bytes) =>
  Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");

const bytes = (h) =>
  new Uint8Array(h.match(/../g).map((b) => parseInt(b, 16)));

// What the host says about its browser plane. Everything here is public.
export async function fetchPlane(mgmtOrigin) {
  const r = await fetch(new URL("/api/v1/webtransport", mgmtOrigin), {
    cache: "no-store",
  });
  if (r.status === 404) throw new Error("this host does not offer the browser plane");
  if (!r.ok) throw new Error(`the host answered ${r.status}`);
  return r.json();
}

// Is this plane's certificate vouched for by the host we paired with?
//
// Two things have to hold and neither alone is worth anything: the certificate must be the one
// whose fingerprint we stored, and it must have signed THIS hash. A host that fails either is
// not dialled — the point is to refuse before the connection, not after.
export async function verify(plane, hostFingerprint) {
  if (!plane.cert_hash_sig || !plane.host_cert_der) {
    throw new Error("this host published no attestation, so a paired browser cannot dial it");
  }
  const der = Uint8Array.from(atob(plane.host_cert_der), (c) => c.charCodeAt(0));
  const seen = hex(new Uint8Array(await crypto.subtle.digest("SHA-256", der)));
  if (seen !== hostFingerprint) {
    throw new Error("this is not the host that was paired with");
  }
  const key = await crypto.subtle.importKey(
    "spki",
    spkiOf(der),
    { name: "ECDSA", namedCurve: "P-256" },
    false,
    ["verify"],
  );
  const ok = await crypto.subtle.verify(
    { name: "ECDSA", hash: "SHA-256" },
    key,
    // The host signs ASN.1 DER; WebCrypto verifies raw `r || s`, the same mismatch the client's
    // own signatures have in the other direction.
    derToRaw(bytes(plane.cert_hash_sig)),
    new TextEncoder().encode(CTX + plane.cert_hash_sha256),
  );
  if (!ok) throw new Error("the host's signature over its certificate hash does not verify");
  return true;
}

// The SubjectPublicKeyInfo inside an X.509 certificate.
//
// A P-256 SPKI is a fixed 91-byte shape, so finding its header is exact rather than a parse: the
// SEQUENCE, both OIDs and the BIT STRING tag are all determined by the key type. Anything else
// is not a certificate we can verify, which is the correct answer for a non-P-256 host.
function spkiOf(der) {
  const header = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08,
    0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
  ];
  for (let i = 0; i + 91 <= der.length; i++) {
    if (header.every((b, j) => der[i + j] === b)) return der.subarray(i, i + 91);
  }
  throw new Error("the host certificate carries no P-256 key");
}

// `SEQUENCE { INTEGER r, INTEGER s }` to the fixed 64 bytes WebCrypto wants. Refuses anything
// that is not exactly that: this parses bytes from an unauthenticated route.
function derToRaw(der) {
  let i = 0;
  const int = () => {
    if (der[i++] !== 0x02) throw new Error("bad signature");
    const n = der[i++];
    if (n & 0x80) throw new Error("bad signature");
    let v = der.subarray(i, (i += n));
    while (v.length && v[0] === 0) v = v.subarray(1);
    if (v.length > 32) throw new Error("bad signature");
    return v;
  };
  if (der[i++] !== 0x30 || der[i++] !== der.length - 2) throw new Error("bad signature");
  const r = int();
  const s = int();
  if (i !== der.length) throw new Error("bad signature");
  const raw = new Uint8Array(64);
  raw.set(r, 32 - r.length);
  raw.set(s, 64 - s.length);
  return raw;
}

// What this browser remembers about a host. `localStorage`, because it must survive the tab and
// there is nothing secret in it — the device key itself lives in IndexedDB, non-extractable.
export const hosts = {
  key: (origin) => `pf.host.${origin}`,
  fingerprint(origin) {
    return localStorage.getItem(hosts.key(origin));
  },
  remember(origin, fingerprint) {
    localStorage.setItem(hosts.key(origin), fingerprint);
  },
  forget(origin) {
    localStorage.removeItem(hosts.key(origin));
  },
};

// The host fingerprint to store at pairing: the identity that signed, not the throwaway plane
// certificate, which is replaced every twelve days.
export async function hostFingerprint(plane) {
  if (!plane.host_cert_der) return null;
  const der = Uint8Array.from(atob(plane.host_cert_der), (c) => c.charCodeAt(0));
  return hex(new Uint8Array(await crypto.subtle.digest("SHA-256", der)));
}
