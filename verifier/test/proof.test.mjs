import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { verifyPageProof } from '../src/proof.mjs';
const fixture = JSON.parse(await readFile(new URL('../../compatibility/fixtures/claim-proof-v1.json', import.meta.url)));
for (const proof of fixture.cases) {
  const expected = { ...proof, expiresAt: BigInt(proof.expiresAt), now: 123455n };
  test(`core proof verifies independently; wallet wrapping=${proof.wrap}`, async () => {
    const result = await verifyPageProof(proof.content, expected);
    assert.equal(result?.digest, proof.proofDigest);
  });
  test(`initial request enforces every binding and expiry; wrapping=${proof.wrap}`, async () => {
    for (const change of [{ page: proof.page + 'x' }, { genesisHash: '0x'+'03'.repeat(32) }, { account: '0x'+'04'.repeat(32) }, { nonce: '0x'+'05'.repeat(32) }, { expiresAt: 123457n }, { now: 123456n }, { now: 999999n }]) {
      assert.equal(await verifyPageProof(proof.content, { ...expected, ...change }), null);
    }
  });
  test(`accepted persistent tag outlives initial expiry, but cannot switch periods/digests; wrapping=${proof.wrap}`, async () => {
    assert.ok(await verifyPageProof(proof.content, { ...expected, now: 999999n, acceptedDigest: proof.proofDigest }));
    assert.equal(await verifyPageProof(proof.content, { ...expected, now: 999999n, acceptedDigest: '0x'+'00'.repeat(32) }), null);
    assert.equal(await verifyPageProof(proof.content, { ...expected, acceptedDigest: proof.proofDigest, account: '0x'+'04'.repeat(32) }), null);
  });
  test(`rejects corrupted and noncanonical envelope encodings; wrapping=${proof.wrap}`, async () => {
    for (const bad of [proof.content+'00', proof.content+'.00', proof.content.replace(':01', ':02'), proof.content.replace(/^wika-claim-v1:/, 'wika-claim-v2:'), proof.content.toUpperCase()]) {
      assert.equal(await verifyPageProof(bad, expected), null);
    }
  });
}
