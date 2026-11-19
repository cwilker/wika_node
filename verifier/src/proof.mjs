import { blake2AsHex, cryptoWaitReady, decodeAddress, signatureVerify } from '@polkadot/util-crypto';
import { u8aToHex } from '@polkadot/util';
const PREFIX = 'wika-claim-v1:';
const DOMAIN = Buffer.from('wika/page-claim/v1\0');
const sameAccount = (a, b) => Buffer.from(decodeAddress(a)).equals(Buffer.from(decodeAddress(b)));

/** Validate against an authoritative node request. Never accept bindings supplied by a webpage.
 * acceptedDigest is only for an already accepted ownership period. Its original initial expiry
 * must not invalidate a persistent tag; the new verifier round has its own nonce/deadline.
 */
export async function verifyPageProof(content, expected) {
  try {
    if (typeof content !== 'string' || content.length > 2048 || !content.startsWith(PREFIX)) return null;
    const parts = content.slice(PREFIX.length).split('.');
    if (parts.length !== 2 || parts.some(p => !/^(?:[0-9a-f]{2})+$/.test(p))) return null;
    const body = Buffer.from(parts[0], 'hex');
    const signature = Buffer.from(parts[1], 'hex');
    if (body[0] !== 1 || body[1] !== 1 || body.length < 108) return null;
    const genesisHash = u8aToHex(body.subarray(2, 34));
    const mode = body[34] & 3;
    if (mode > 1) return null; // URLs are at most 254 bytes, hence only 1/2-byte compact lengths.
    const size = mode === 0 ? body[34] >> 2 : body.readUInt16LE(34) >> 2;
    if (size === 0 || size > 254 || mode === 1 && size < 64) return null;
    const start = mode === 0 ? 35 : 36;
    if (body.length !== start + size + 32 + 32 + 8) return null;
    const pageBytes = body.subarray(start, start + size);
    if ([...pageBytes].some(b => b < 33 || b > 126)) return null;
    const page = pageBytes.toString('ascii');
    const account = u8aToHex(body.subarray(start + size, start + size + 32));
    const nonce = u8aToHex(body.subarray(start + size + 32, start + size + 64));
    const expiresAt = body.readBigUInt64LE(start + size + 64);
    if (expiresAt === 0n || page !== expected.page || genesisHash !== expected.genesisHash || !sameAccount(account, expected.account)) return null;
    // The caller's page is the canonical on-chain identity, not an untrusted URL alias.
    const url = new URL(page);
    if (url.protocol !== 'https:' || url.port || url.username || url.password || url.hash) return null;
    await cryptoWaitReady();
    const digest = blake2AsHex(content, 256);
    if (expected.acceptedDigest) {
      if (digest !== expected.acceptedDigest) return null;
    } else if (nonce !== expected.nonce || expiresAt !== expected.expiresAt || BigInt(expected.now) >= expiresAt) return null;
    const kind = signature[0];
    if (kind > 2 || signature.length !== (kind === 2 ? 66 : 65)) return null;
    const result = signatureVerify(Buffer.concat([DOMAIN, body]), signature.subarray(1), account);
    if (!result.isValid || result.crypto !== ['ed25519', 'sr25519', 'ecdsa'][kind]) return null;
    return { page, account, genesisHash, nonce, expiresAt, digest };
  } catch { return null; }
}
