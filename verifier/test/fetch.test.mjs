import { test, afterEach, mock } from 'node:test';
import assert from 'node:assert/strict';
import dns from 'node:dns/promises';
import https from 'node:https';
import { EventEmitter } from 'node:events';
import { fetchPage, isPublicAddress, FETCH_LIMITS } from '../src/fetch.mjs';
afterEach(() => mock.restoreAll());
for (const address of ['127.0.0.1', '0.0.0.0', '10.0.0.1', '172.16.0.1', '192.168.1.1', '169.254.169.254', '100.64.0.1', '224.0.0.1', '192.0.2.1', '198.18.0.1', '255.255.255.255', '::', '::1', 'fe80::1', 'fc00::1', '2001:db8::1', '::ffff:8.8.8.8', '2002:0808:0808::1', 'fe80::1%lo0', 'not-an-ip']) {
  test(`rejects destination ${address}`, () => assert.equal(isPublicAddress(address), false));
}
test('permits ordinary public IPv4 and native IPv6 destinations', () => {
  for (const ip of ['8.8.8.8', '1.1.1.1', '2606:4700:4700::1111']) assert.equal(isPublicAddress(ip), true);
});
function transport({ status = 200, headers = { 'content-type': 'text/html; charset=utf-8' }, chunks = [Buffer.from('<html/>')], peer = '8.8.8.8' } = {}) {
  const lookups = mock.method(dns, 'lookup', async () => [{ address: '8.8.8.8', family: 4 }]);
  const requests = mock.method(https, 'request', (url, options, onResponse) => {
    assert.equal(url.hostname, 'example.com');
    assert.equal(options.agent, false);
    assert.equal(options.autoSelectFamily, false);
    assert.equal(options.rejectUnauthorized, undefined); // Node's strict TLS default stays enabled.
    options.lookup('example.com', {}, (error, address, family) => {
      assert.equal(error, null); assert.equal(address, '8.8.8.8'); assert.equal(family, 4);
    });
    const request = new EventEmitter();
    request.destroy = error => { request.destroyed = true; request.emit('error', error); };
    request.end = () => queueMicrotask(() => {
      const socket = new EventEmitter(); socket.remoteAddress = peer;
      request.emit('socket', socket); socket.emit('secureConnect');
      if (request.destroyed) return;
      const response = new EventEmitter();
      response.statusCode = status; response.headers = headers; response.complete = true;
      response.destroy = error => { response.destroyed = true; if (error) response.emit('error', error); };
      onResponse(response);
      if (response.destroyed) return;
      for (const chunk of chunks) { response.emit('data', chunk); if (response.destroyed) return; }
      response.emit('end');
    });
    return request;
  });
  return { lookups, requests };
}
test('pins validated DNS, checks the actual peer, retains hostname TLS and returns bounded UTF-8 HTML', async () => {
  const { lookups, requests } = transport();
  assert.equal(await fetchPage('https://example.com/'), '<html/>');
  assert.equal(lookups.mock.calls.length, 1);
  assert.equal(requests.mock.calls.length, 1);
});
test('rejects public/private mixed DNS without opening any socket', async () => {
  mock.method(dns, 'lookup', async () => [{ address: '8.8.8.8', family: 4 }, { address: '127.0.0.1', family: 4 }]);
  const request = mock.method(https, 'request', () => assert.fail('must not connect'));
  await assert.rejects(fetchPage('https://example.com/'), /Non-public/);
  assert.equal(request.mock.calls.length, 0);
});
for (const status of [301, 302, 303, 307, 308, 404, 500]) test(`does not follow HTTP ${status}`, async () => {
  const { requests } = transport({ status, headers: { location: 'https://127.0.0.1/' } });
  await assert.rejects(fetchPage('https://example.com/'), /HTTP 200/);
  assert.equal(requests.mock.calls.length, 1);
});
for (const [name, options] of Object.entries({
  wrongPeer: { peer: '1.1.1.1' }, privatePeer: { peer: '127.0.0.1' }, substringPeer: { peer: '18.8.8.8' },
  compressed: { headers: { 'content-encoding': 'gzip', 'content-type': 'text/html' } },
  nonHtml: { headers: { 'content-type': 'application/json' } },
  wrongCharset: { headers: { 'content-type': 'text/html; charset=iso-8859-1' } },
  invalidUtf8: { chunks: [Buffer.from([0xff])] },
  oversized: { chunks: [Buffer.alloc(FETCH_LIMITS.maxBytes), Buffer.from('x')] },
  declaredOversized: { headers: { 'content-type': 'text/html', 'content-length': String(FETCH_LIMITS.maxBytes + 1) } },
})) test(`rejects ${name}`, async () => { transport(options); await assert.rejects(fetchPage('https://example.com/')); });
for (const url of ['http://example.com/', 'https://user:pass@example.com/', 'https://example.com:8443/', 'https://example.com/#a', 'https://example.com\\@evil.com/']) test(`rejects fetch URL ${url}`, async () => {
  await assert.rejects(fetchPage(url));
});
test('an already cancelled fetch does not resolve or connect', async () => {
  mock.method(dns, 'lookup', () => assert.fail('must not resolve'));
  const controller = new AbortController(); controller.abort();
  await assert.rejects(fetchPage('https://example.com/', { signal: controller.signal }));
});

test('cancellation interrupts DNS without opening a socket', async () => {
  mock.method(dns, 'lookup', () => new Promise(() => {}));
  const request = mock.method(https, 'request', () => assert.fail('must not connect'));
  const controller = new AbortController();
  const result = fetchPage('https://example.com/', { signal: controller.signal });
  controller.abort(new Error('Cancelled during DNS'));
  await assert.rejects(result, /Cancelled during DNS/);
  assert.equal(request.mock.calls.length, 0);
});
