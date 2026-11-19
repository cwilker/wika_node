/** Real local DNS, TCP, TLS and HTTP parsing. Only DNS API selection, TCP routing,
 * test CA trust and (where stated) the observed public peer are adapted in this
 * test process. Production fetch code has no test mode or private-network escape.
 * This does not validate OS routing/firewalls or a production recursive resolver.
 */
import { test, before, after, afterEach, mock } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import dns, { Resolver } from 'node:dns/promises';
import dgram from 'node:dgram';
import https from 'node:https';
import tls from 'node:tls';
import { once } from 'node:events';
import { fetchPage, FetchUnavailable, FETCH_LIMITS } from '../src/fetch.mjs';

const namespace = process.env.WIKA_TRANSPORT_NETWORK_NAMESPACE === '1';
const hostname = 'fetch.wika.test';
const publicAddress = '8.8.8.8'; // Isolated namespace alias or modeled label; never the external host.
const originalRequest = https.request;
let directory, ca, valid, wrongName, expired;
const cleanups = [];
function openssl(args) { execFileSync('openssl', args, { cwd: directory, stdio: 'pipe' }); }
function certificate(name, san, days = '1') {
  openssl(['req', '-new', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:P-256', '-nodes',
    '-keyout', `${name}.key`, '-out', `${name}.csr`, '-subj', `/CN=${san}`]);
  writeFileSync(join(directory, `${name}.ext`), `subjectAltName=DNS:${san}\nextendedKeyUsage=serverAuth\nbasicConstraints=CA:FALSE\n`);
  if (days === '-1') {
    writeFileSync(join(directory, 'index.txt'), '');
    writeFileSync(join(directory, 'serial'), '10');
    writeFileSync(join(directory, 'ca.cnf'), '[ca]\ndefault_ca=issuer\n[issuer]\ndatabase=index.txt\nserial=serial\nnew_certs_dir=.\ncertificate=ca.pem\nprivate_key=ca.key\ndefault_md=sha256\npolicy=policy\n[policy]\ncommonName=supplied\n');
    openssl(['ca', '-batch', '-config', 'ca.cnf', '-in', `${name}.csr`, '-out', `${name}.pem`,
      '-startdate', '20000101000000Z', '-enddate', '20000102000000Z', '-extfile', `${name}.ext`]);
  } else openssl(['x509', '-req', '-in', `${name}.csr`, '-CA', 'ca.pem', '-CAkey', 'ca.key',
    '-set_serial', String({ valid: 2, wrong: 3, expired: 4 }[name]), '-out', `${name}.pem`,
    '-days', days, '-extfile', `${name}.ext`]);
  return { key: readFileSync(join(directory, `${name}.key`)), cert: readFileSync(join(directory, `${name}.pem`)) };
}
before(() => {
  directory = mkdtempSync(join(tmpdir(), 'wika-transport-'));
  openssl(['req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:P-256', '-nodes',
    '-keyout', 'ca.key', '-out', 'ca.pem', '-days', '1', '-subj', '/CN=Wika disposable test CA',
    '-addext', 'basicConstraints=critical,CA:TRUE']);
  ca = readFileSync(join(directory, 'ca.pem'));
  valid = certificate('valid', hostname);
  wrongName = certificate('wrong', 'different.wika.test');
  expired = certificate('expired', hostname, '-1');
});
afterEach(async () => {
  mock.restoreAll();
  for (const cleanup of cleanups.splice(0).reverse()) await cleanup();
});
after(() => { if (directory) rmSync(directory, { recursive: true, force: true }); });

// Minimal authoritative UDP DNS fixture. The real c-ares Resolver sends/decodes
// packets; the adapter replaces OS dns.lookup, not the production pinning logic.
async function authority(answerSets = [[publicAddress]], silent = false) {
  const socket = dgram.createSocket('udp4');
  let queries = 0;
  socket.on('message', (packet, remote) => {
    if (silent) { queries++; return; }
    let end = 12;
    while (packet[end]) end += packet[end] + 1;
    end += 5;
    const answers = answerSets[Math.min(queries++, answerSets.length - 1)];
    const header = Buffer.alloc(12);
    packet.copy(header, 0, 0, 2);
    header.writeUInt16BE(0x8180, 2); header.writeUInt16BE(1, 4); header.writeUInt16BE(answers.length, 6);
    const records = answers.map(address => {
      const record = Buffer.from('c00c00010001000000000004' + address.split('.').map(x => Number(x).toString(16).padStart(2, '0')).join(''), 'hex');
      return record;
    });
    socket.send(Buffer.concat([header, packet.subarray(12, end), ...records]), remote.port, remote.address);
  });
  socket.bind(0, '127.0.0.1'); await once(socket, 'listening');
  cleanups.push(() => new Promise(resolve => socket.close(resolve)));
  const resolver = new Resolver({ timeout: 1000, tries: 1 });
  resolver.setServers([`127.0.0.1:${socket.address().port}`]);
  cleanups.push(() => resolver.cancel());
  const lookup = mock.method(dns, 'lookup', async (name, options) => {
    assert.equal(name, hostname); assert.equal(options.all, true);
    return (await resolver.resolve4(name)).map(address => ({ address, family: 4 }));
  });
  return { get queries() { return queries; }, lookup };
}

async function origin(handler = (_request, response) => {
  response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' }); response.end('<html>safe</html>');
}, cert = valid, raw = false) {
  const sockets = new Set();
  const server = raw ? tls.createServer(cert, handler) : https.createServer(cert, handler);
  server.on('connection', socket => { sockets.add(socket); socket.once('close', () => sockets.delete(socket)); });
  server.on('tlsClientError', () => {});
  server.listen(namespace ? 443 : 0, namespace ? '0.0.0.0' : '127.0.0.1'); await once(server, 'listening');
  cleanups.push(() => new Promise(resolve => { for (const socket of sockets) socket.destroy(); server.close(resolve); }));
  return server;
}

function route(server, { peer = publicAddress, trust = true } = {}) {
  const observed = { requests: 0, pins: [], authorized: [], servernames: [] };
  mock.method(https, 'request', (url, options, callback) => {
    observed.requests++;
    assert.equal(url.hostname, hostname); assert.equal(url.port, '');
    assert.equal(options.agent, false); assert.equal(options.autoSelectFamily, false);
    assert.equal(options.rejectUnauthorized, undefined);
    assert.equal(options.checkServerIdentity, undefined);
    const request = originalRequest(url, {
      ...options, ...(namespace ? {} : { port: server.address().port }), ...(trust ? { ca } : {}),
      lookup: (name, lookupOptions, done) => options.lookup(name, lookupOptions, (error, address, family) => {
        assert.ifError(error);
        const pin = Array.isArray(address) ? address[0] : { address, family };
        observed.pins.push(pin);
        assert.equal(pin.address, publicAddress); assert.equal(pin.family, 4);
        // Namespace aliases exist only on its isolated loopback interface.
        const destination = namespace ? (peer ?? '127.0.0.1') : '127.0.0.1';
        if (lookupOptions?.all) done(null, [{ address: destination, family: 4 }]);
        else done(null, destination, 4);
      }),
    }, callback);
    request.prependListener('socket', socket => {
      if (!namespace && peer !== null) Object.defineProperty(socket, 'remoteAddress', { get: () => peer });
      socket.prependOnceListener('secureConnect', () => {
        observed.authorized.push(socket.authorized); observed.servernames.push(socket.servername);
      });
    });
    return request;
  });
  return observed;
}
async function rejected(promise, pattern) {
  await assert.rejects(promise, error => {
    assert.ok(error instanceof FetchUnavailable, error.stack);
    assert.match(error.message, pattern); return true;
  });
}

test('real DNS + trusted TLS: hostname, SNI, pin and HTML survive a rebinding answer', async () => {
  const dnsFixture = await authority([[publicAddress], ['127.0.0.1']]);
  let host;
  const server = await origin((request, response) => {
    host = request.headers.host;
    response.writeHead(200, { 'content-type': 'text/html' }); response.end('<html>safe</html>');
  });
  const observed = route(server);
  assert.equal(await fetchPage(`https://${hostname}/proof`), '<html>safe</html>');
  assert.deepEqual(observed.authorized, [true]); assert.deepEqual(observed.servernames, [hostname]);
  assert.equal(host, namespace ? hostname : `${hostname}:${server.address().port}`);
  assert.equal(dnsFixture.queries, 1); assert.equal(dnsFixture.lookup.mock.calls.length, 1);
  assert.deepEqual(observed.pins, [{ address: publicAddress, family: 4 }]);
  // A later fetch must resolve again and block the newly private destination.
  await rejected(fetchPage(`https://${hostname}/proof`), /Non-public DNS/);
  assert.equal(dnsFixture.queries, 2); assert.equal(observed.requests, 1);
});
for (const addresses of [['127.0.0.1'], [publicAddress, '169.254.169.254'], [publicAddress, '10.0.0.1']]) {
  test(`real DNS blocks ${addresses.join(', ')} before creating a request`, async () => {
    const fixture = await authority([addresses]);
    const request = mock.method(https, 'request', () => assert.fail('must not open a connection'));
    await rejected(fetchPage(`https://${hostname}/`), /Non-public DNS/);
    assert.equal(fixture.queries, 1); assert.equal(request.mock.calls.length, 0);
  });
}
for (const [name, getCert, trust, pattern] of [
  ['untrusted certificate', () => valid, false, /certificate|issuer|self.signed/i],
  ['wrong certificate hostname', () => wrongName, true, /hostname|altnames/i],
  ['expired certificate', () => expired, true, /expired/i],
]) test(`real TLS rejects ${name}`, async () => {
  await authority(); let requests = 0;
  const server = await origin((_req, res) => { requests++; res.end(); }, getCert());
  const observed = route(server, { trust });
  await rejected(fetchPage(`https://${hostname}/`), pattern);
  assert.equal(requests, 0); assert.deepEqual(observed.authorized, []);
});
for (const [name, peer] of [['actual loopback peer', null], ['different public peer', '1.1.1.1']]) {
  test(`real TLS connection rejects ${name}`, async () => {
    await authority(); const server = await origin();
    route(server, { peer });
    await rejected(fetchPage(`https://${hostname}/`), /destination.*pinned/);
  });
}
for (const [name, handler, pattern] of [
  ['redirect to metadata service', (_q, r) => { r.writeHead(302, { location: 'http://169.254.169.254/latest/meta-data/' }); r.end(); }, /redirects/],
  ['compressed body', (_q, r) => { r.writeHead(200, { 'content-type': 'text/html', 'content-encoding': 'gzip' }); r.end('invalid gzip'); }, /Compressed/],
  ['invalid UTF-8', (_q, r) => { r.writeHead(200, { 'content-type': 'text/html' }); r.end(Buffer.from([0xff])); }, /UTF-8/],
  ['oversized declared body', (_q, r) => { r.writeHead(200, { 'content-type': 'text/html', 'content-length': FETCH_LIMITS.maxBytes + 1 }); r.flushHeaders(); }, /byte limit/],
  ['oversized chunked body', (_q, r) => { r.writeHead(200, { 'content-type': 'text/html' }); r.end(Buffer.alloc(FETCH_LIMITS.maxBytes + 1)); }, /byte limit/],
]) test(`real HTTPS rejects ${name}`, async () => {
  const fixture = await authority(); const server = await origin(handler); const observed = route(server);
  await rejected(fetchPage(`https://${hostname}/`), pattern);
  assert.equal(observed.requests, 1); assert.equal(fixture.queries, 1);
});
for (const [name, handler, pattern] of [
  ['oversized headers', socket => socket.end('HTTP/1.1 200 OK\r\nX-Large: ' + 'x'.repeat(FETCH_LIMITS.maxHeaderBytes) + '\r\n\r\n'), /Header overflow/],
  ['truncated response', socket => socket.end('HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 100\r\n\r\nshort'), /aborted|Incomplete|reset/i],
]) test(`real TLS wire rejects ${name}`, async () => {
  await authority(); const server = await origin(handler, valid, true); route(server);
  await rejected(fetchPage(`https://${hostname}/`), pattern);
});
test('cancellation interrupts a real stalled TLS handshake', async () => {
  await authority();
  // TCP server accepts without ever returning a TLS ServerHello.
  const net = await import('node:net');
  const sockets = new Set();
  const server = net.createServer(socket => { sockets.add(socket); socket.once('close', () => sockets.delete(socket)); });
  server.listen(namespace ? 443 : 0, namespace ? '0.0.0.0' : '127.0.0.1'); await once(server, 'listening');
  cleanups.push(() => new Promise(resolve => { for (const socket of sockets) socket.destroy(); server.close(resolve); }));
  route(server);
  const controller = new AbortController(); const connected = once(server, 'connection');
  const result = fetchPage(`https://${hostname}/`, { signal: controller.signal });
  await connected; controller.abort();
  await rejected(result, /abort/i);
});
test('total deadline closes a real HTTPS origin that never sends headers', { timeout: 20_000 }, async () => {
  await authority(); const server = await origin(() => {}); route(server);
  const started = performance.now();
  await rejected(fetchPage(`https://${hostname}/`), /abort|deadline/i);
  assert.ok(performance.now() - started < FETCH_LIMITS.timeoutMs + 3000);
});

test('cancellation interrupts an unanswered real DNS query without connecting', { timeout: 5000 }, async () => {
  const fixture = await authority([[publicAddress]], true);
  const request = mock.method(https, 'request', () => assert.fail('must not connect'));
  const controller = new AbortController();
  const result = fetchPage(`https://${hostname}/`, { signal: controller.signal });
  while (!fixture.queries) await new Promise(resolve => setTimeout(resolve, 5));
  controller.abort(new Error('Cancelled during DNS'));
  await rejected(result, /Cancelled during DNS/);
  assert.equal(request.mock.calls.length, 0);
});
