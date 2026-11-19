import https from 'node:https';
import dns from 'node:dns/promises';
import { isIP } from 'node:net';
import ipaddr from 'ipaddr.js';

export const FETCH_LIMITS = Object.freeze({ timeoutMs: 15_000, maxBytes: 2 * 1024 * 1024, maxHeaderBytes: 16 * 1024 });
export class FetchUnavailable extends Error {}

/** Allow only globally routable destinations. IPv4 mapped/transition IPv6 is rejected. */
export function isPublicAddress(address) {
  try {
    if (!isIP(address) || address.includes('%')) return false;
    const ip = ipaddr.parse(address);
    if (ip.range() !== 'unicast') return false;
    if (ip.kind() === 'ipv6' && !ip.match(ipaddr.parse('2000::'), 3)) return false;
    return true;
  } catch { return false; }
}

function validateUrl(value) {
  if (typeof value !== 'string' || value.length > 254 || /[^\x21-\x7e]|[\\]/.test(value)) throw new FetchUnavailable('Invalid fetch URL');
  let url;
  try { url = new URL(value); } catch { throw new FetchUnavailable('Invalid fetch URL'); }
  if (url.protocol !== 'https:' || url.username || url.password || url.hash || url.port) throw new FetchUnavailable('Only credential-free HTTPS on port 443 is supported');
  if (url.hostname.endsWith('.') || url.hostname.includes('%')) throw new FetchUnavailable('Ambiguous hostname');
  return url;
}

/** Resolve once, validate every answer, and use that exact address in the connection lookup.
 * No environment proxy, redirects, connection pool, implicit decompression or second DNS lookup.
 * TLS still verifies the original hostname with the system trust store.
 */
export async function fetchPage(value, { signal } = {}) {
  const url = validateUrl(value);
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(new FetchUnavailable('Fetch deadline exceeded')), FETCH_LIMITS.timeoutMs);
  const aborted = () => controller.abort(signal.reason);
  signal?.addEventListener('abort', aborted, { once: true });
  if (signal?.aborted) aborted();
  try {
    if (controller.signal.aborted) throw controller.signal.reason;
    // DNS is inside the same total deadline as TCP, TLS, headers and body.
    const addresses = await Promise.race([
      dns.lookup(url.hostname, { all: true, verbatim: true }),
      new Promise((_, reject) => controller.signal.addEventListener('abort', () => reject(controller.signal.reason), { once: true })),
    ]);
    if (!addresses.length || addresses.some(({ address }) => !isPublicAddress(address))) throw new FetchUnavailable('Non-public DNS destination');
    const pinned = addresses[0];
    return await requestPinned(url, pinned, controller.signal);
  } catch (error) {
    throw error instanceof FetchUnavailable ? error : new FetchUnavailable(error?.message ?? 'Fetch unavailable');
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', aborted);
  }
}

function requestPinned(url, pinned, signal) {
  return new Promise((resolve, reject) => {
    const fail = (message) => new FetchUnavailable(message);
    const request = https.request(url, {
      method: 'GET', agent: false, signal,
      // Disable any second resolution, including requests from newer auto family selection.
      autoSelectFamily: false,
      lookup: (_hostname, options, callback) => {
        if (options?.all) callback(null, [pinned]);
        else callback(null, pinned.address, pinned.family);
      },
      maxHeaderSize: FETCH_LIMITS.maxHeaderBytes,
      headers: { Accept: 'text/html', 'Accept-Encoding': 'identity', 'User-Agent': 'WikaClaimVerifier/1' },
    }, response => {
      // A redirect is unavailable evidence, never proof of an invalid claim.
      if (response.statusCode !== 200) { response.destroy(); reject(fail('Expected HTTP 200; redirects are forbidden')); return; }
      if (response.headers['content-encoding'] && response.headers['content-encoding'].toLowerCase() !== 'identity') { response.destroy(); reject(fail('Compressed responses are unsupported')); return; }
      if (!/^text\/html(?:\s*;|\s*$)/i.test(response.headers['content-type'] ?? '')) { response.destroy(); reject(fail('Expected text/html')); return; }
      const charset = /charset\s*=\s*([^;]+)/i.exec(response.headers['content-type'] ?? '')?.[1].trim().replace(/^"|"$/g, '').toLowerCase();
      if (charset && charset !== 'utf-8' && charset !== 'utf8' && charset !== 'us-ascii') { response.destroy(); reject(fail('Unsupported HTML charset')); return; }
      const length = response.headers['content-length'];
      if (length && (!/^\d+$/.test(length) || Number(length) > FETCH_LIMITS.maxBytes)) { response.destroy(); reject(fail('Response exceeds byte limit')); return; }
      let count = 0;
      const chunks = [];
      response.on('data', chunk => {
        count += chunk.length;
        if (count > FETCH_LIMITS.maxBytes) { response.destroy(fail('Response exceeds byte limit')); return; }
        chunks.push(chunk);
      });
      response.once('error', reject);
      response.once('end', () => {
        if (!response.complete) { reject(fail('Incomplete response')); return; }
        try { resolve(new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks))); }
        catch { reject(fail('Invalid UTF-8 HTML')); }
      });
    });
    request.on('socket', socket => socket.once('secureConnect', () => {
      // Lookup pinning is authoritative; this is a defense-in-depth check on the actual peer.
      try {
        if (!isPublicAddress(socket.remoteAddress) || ipaddr.parse(socket.remoteAddress).toNormalizedString() !== ipaddr.parse(pinned.address).toNormalizedString()) {
          request.destroy(fail('Connection destination did not match pinned address'));
        }
      } catch { request.destroy(fail('Invalid connection destination')); }
    }));
    request.once('error', reject);
    request.end();
  });
}
