// The browser's half of the attestation check, against bytes a real host produced.
//
// This is the one place the two languages have to agree byte for byte, and the failure mode if
// they do not is silent: a paired browser that refuses every host, or worse, one that accepts a
// signature it should not. `node --test clients/web/web/` runs it, no dependencies.
//
// The vector below came from `webtransport::attest` on a freshly minted host identity. It is a
// public certificate and a signature over a fixed hash — nothing secret, and nothing that
// authorises anything.

import { test } from "node:test";
import assert from "node:assert/strict";
import { verify, hostFingerprint } from "./pf-connect.js";

const VECTOR = {
  cert_hash_sha256: "3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f",
  cert_hash_sig:
    "30450220363b8bd78823a5b83ce942385c0d2a129aaea5d5d551252e09ee76355efa47d0022100f22d19aeef15ba9fe8b1b270cf8e217e298cfb7eb95e4c7f059070db7dfffa4e",
  host_cert_der:
    "MIIBajCCARCgAwIBAgIUBHNzKP9D1XbUY4sWH3W71dYNpI0wCgYIKoZIzj0EAwIwFDESMBAGA1UEAwwJcHVua3RmdW5rMB4XDTIwMDEwMTAwMDAwMFoXDTQwMDEwMTAwMDAwMFowFDESMBAGA1UEAwwJcHVua3RmdW5rMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAERDhsbozwd9pED0lmfGpj4T0BDZCJD0alv3xEUWcy11JDOzNquH2WhXdEKpgyIKH0iC+q6f4eRqTBSpsadKEHJ6NAMD4wPAYDVR0RBDUwM4IOcHVua3RmdW5rLWhvc3SCCWxvY2FsaG9zdIcEfwAAAYcQAAAAAAAAAAAAAAAAAAAAATAKBggqhkjOPQQDAgNIADBFAiEAxMl/ycm/SSIIUG7GXQ9luBSgzhfbXScsyDYPYmGyLjsCIEWoxZX9UipHzlkMbyCE3AeHjtye8mx7twJCef4Eg3NO",
};

test("a real host's attestation verifies", async () => {
  const fp = await hostFingerprint(VECTOR);
  assert.equal(fp.length, 64, "a SHA-256 in hex");
  assert.equal(await verify(VECTOR, fp), true);
});

test("a host that is not the one paired with is refused", async () => {
  await assert.rejects(
    () => verify(VECTOR, "0".repeat(64)),
    /not the host that was paired with/,
  );
});

test("the signature must be over this hash, by this key", async () => {
  const fp = await hostFingerprint(VECTOR);

  // A different hash: the certificate is still ours, the signature no longer names it.
  await assert.rejects(
    () => verify({ ...VECTOR, cert_hash_sha256: "ab".repeat(32) }, fp),
    /does not verify/,
  );

  // A bent signature. `atob`-clean, structurally valid, wrong.
  const bent = VECTOR.cert_hash_sig.slice(0, -2) + (VECTOR.cert_hash_sig.endsWith("00") ? "01" : "00");
  await assert.rejects(() => verify({ ...VECTOR, cert_hash_sig: bent }, fp), /verify|signature/);
});

test("an unattested host cannot be dialled by a paired browser", async () => {
  const fp = await hostFingerprint(VECTOR);
  await assert.rejects(
    () => verify({ cert_hash_sha256: VECTOR.cert_hash_sha256 }, fp),
    /published no attestation/,
  );
});
