// Independent development-only encoder; production clients use @wika/core.
import { blake2AsHex } from '@polkadot/util-crypto';
import { hexToU8a, u8aToHex } from '@polkadot/util';

export function fixtureProof(pair, api, page, expiresAt) {
  const url = new TextEncoder().encode(page);
  const compact = url.length < 64 ? [url.length << 2] : [((url.length << 2) | 1) & 255, url.length >> 6];
  const expiry = new Uint8Array(8);
  new DataView(expiry.buffer).setBigUint64(0, expiresAt, true);
  const body = Uint8Array.from([1, 1, ...hexToU8a(api.genesisHash.toHex()), ...compact, ...url,
    ...pair.publicKey, ...crypto.getRandomValues(new Uint8Array(32)), ...expiry]);
  const message = Uint8Array.from([...new TextEncoder().encode('wika/page-claim/v1\0'), ...body]);
  const content = `wika-claim-v1:${u8aToHex(body).slice(2)}.01${u8aToHex(pair.sign(message)).slice(2)}`;
  return { page, content, proofDigest: blake2AsHex(content, 256) };
}
