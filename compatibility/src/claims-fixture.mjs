// Development-only website fixture; the worker itself enforces development-chain use.
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { Keyring } from '@polkadot/api';
import { blake2AsHex, cryptoWaitReady } from '@polkadot/util-crypto';
import { hexToU8a, u8aToHex } from '@polkadot/util';
import { startVerifier } from '../../verifier/src/worker.mjs';
// Independent test encoder: production clients use @wika/core, the verifier decodes and checks this envelope.
function fixtureProof(pair, api, page, expiresAt) {
  const url = new TextEncoder().encode(page);
  const compact = url.length < 64 ? [url.length << 2] : [((url.length << 2) | 1) & 255, url.length >> 6];
  const expiry = new Uint8Array(8); new DataView(expiry.buffer).setBigUint64(0, expiresAt, true);
  const body = Uint8Array.from([1, 1, ...hexToU8a(api.genesisHash.toHex()), ...compact, ...url, ...pair.publicKey, ...crypto.getRandomValues(new Uint8Array(32)), ...expiry]);
  const message = Uint8Array.from([...new TextEncoder().encode('wika/page-claim/v1\0'), ...body]);
  const content = `wika-claim-v1:${u8aToHex(body).slice(2)}.01${u8aToHex(pair.sign(message)).slice(2)}`;
  return { page, content, proofDigest: blake2AsHex(content,256) };
}
export async function claimFixture(api, directory, page) {
  if (!(await api.rpc.system.chainType()).isDevelopment) throw new Error('Fixture requires isolated development chain');
  await cryptoWaitReady();
  const keys = new Keyring({ type: 'sr25519' });
  const alice = keys.addFromUri('//Alice');
  const fixtures = join(directory, 'pages.json');
  await writeFile(fixtures, '{}');
  const submit = (tx, pair = alice) => new Promise((resolve, reject) => {
    let stop, done = false;
    const timer = setTimeout(() => { done = true; stop?.(); reject(new Error('Fixture transaction finality timeout')); }, 90000);
    tx.signAndSend(pair, { nonce: -1 }, result => {
      if (!result.status.isFinalized || done) return;
      done = true; clearTimeout(timer); stop?.();
      if (result.dispatchError) reject(new Error(result.dispatchError.toString())); else resolve(result);
    }).then(unsub => { stop = unsub; if (done) unsub(); }, err => { clearTimeout(timer); reject(err); });
  });
  await submit(api.tx.sudo.sudo(api.tx.system.setStorage([
    [api.query.owners.numBlocksToCommit.key(), '0x02'],
    [api.query.owners.numBlocksToReveal.key(), '0x02'],
  ])));
  const worker = await startVerifier({ api, pair: keys.addFromUri('//Alice//ownr'), stateFile: join(directory, 'reviewer.json'), fixtures, log: message => console.log(`[claim fixture] ${message}`) });
  return {
    submit,
    publish: proof => writeFile(fixtures, JSON.stringify({ [page]: `<html><head><meta name="wika-claim" content="${proof.content}"></head><body>Fixture</body></html>` })),
    proof: async pair => fixtureProof(pair, api, page, BigInt((await api.query.timestamp.now()).toString()) + 3600000n),
    stop: () => worker.stop(),
  };
}
