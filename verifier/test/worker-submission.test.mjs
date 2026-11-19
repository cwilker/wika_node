import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { setImmediate as immediate } from 'node:timers/promises';
import { TypeRegistry } from '@polkadot/types';
import { Keyring } from '@polkadot/api';
import { cryptoWaitReady } from '@polkadot/util-crypto';
import { startVerifier } from '../src/worker.mjs';

async function until(predicate, message) {
  const deadline = performance.now() + 5000;
  while (!predicate()) {
    assert.ok(performance.now() < deadline, message);
    await immediate();
  }
}

// Real ballot encoding/signing and durable files, with a controlled chain API.
// Callbacks deliberately race the returned unsubscribe promise as the real API can.
async function fixture(t, { count = 1, submit } = {}) {
  await cryptoWaitReady();
  const directory = await mkdtemp(join(tmpdir(), 'wika-submission-test-'));
  const pair = new Keyring({ type: 'sr25519' }).addFromUri('//Alice');
  const registry = new TypeRegistry();
  const none = { isNone: true, isSome: false };
  const some = value => ({ isNone: false, isSome: true, unwrap: () => value });
  const integer = value => registry.createType('u32', value);
  const at = integer(1), account = registry.createType('AccountId', pair.address);
  const pages = Array.from({ length: count }, (_, i) => `https://load.wika.test/${i}`);
  const fixtures = join(directory, 'fixtures.json');
  await writeFile(fixtures, JSON.stringify(Object.fromEntries(pages.map(page => [page, '<html><head></head><body>No proof</body></html>']))));
  const context = {
    proof: registry.createType('Bytes', [1]), expiresAt: registry.createType('u64', 999999999),
    toU8a: () => registry.createType('(u8,Bytes,u64,u8)', [0, [1], 999999999, 0]).toU8a(),
  };
  let height = 2, onHead, calls = 0, releases = 0, active = 0, peak = 0;
  const commits = new Map(), logs = [];
  const header = () => ({ number: integer(height), hash: `0xblock${height}` });
  const api = {
    registry, genesisHash: registry.createType('H256', '0x' + '01'.repeat(32)),
    query: { owners: {
      claimChecks() {}, verifiers: async () => some([null, { isTrue: true }]),
    } },
    rpc: {
      system: { chainType: async () => ({ isDevelopment: true }) },
      chain: { subscribeNewHeads: async callback => { onHead = callback; return () => {}; }, getHeader: async () => header() },
    },
    at: async () => ({ query: {
      timestamp: { now: async () => 1000 },
      owners: {
        requests: { entries: async () => pages.map(page => [{ args: [registry.createType('Bytes', Array.from(Buffer.from(page)))] }, some([at, account])]) },
        claimChecks: async () => some(context),
        rounds: async () => some({ members: [account], commitEnd: integer(1000), revealEnd: integer(2000) }),
        commits: async page => commits.has(page) ? some(registry.createType('H256', commits.get(page))) : none,
        reveals: async () => none,
      },
    } }),
    tx: { owners: { commitVerification: (page, hash) => ({
      signAndSend: (_pair, options, callback) => {
        assert.equal(options.nonce, -1);
        calls++; active++; peak = Math.max(peak, active);
        let released = false;
        const unsubscribe = () => { assert.equal(released, false, 'unsubscribe is called only once'); released = true; releases++; active--; };
        const accepted = () => commits.set(page, hash);
        return submit({ callback, unsubscribe, accepted });
      },
    }) } },
  };
  let worker;
  t.after(async () => { await worker?.stop(); await rm(directory, { recursive: true, force: true }); });
  worker = await startVerifier({ api, pair, stateFile: join(directory, 'state.json'), fixtures, log: message => logs.push(message) });
  return {
    worker, logs,
    get calls() { return calls; }, get releases() { return releases; },
    get active() { return active; }, get peak() { return peak; },
    nextHead() { height++; onHead(header()); },
  };
}

test('a full 256-request queue releases subscriptions when terminal status arrives before unsubscribe', async t => {
  const f = await fixture(t, { count: 256, submit: ({ callback, unsubscribe, accepted }) => {
    accepted(); callback({ status: { isInBlock: true } });
    callback({ status: { isFinalized: true } });
    return Promise.resolve(unsubscribe);
  } });
  for (let batch = 1; batch <= 64; batch++) {
    await until(() => f.calls >= batch * 4, 'bounded batch should submit');
    await immediate();
    assert.equal(f.calls, batch * 4, 'only four new observations per tick');
    assert.equal(f.active, 0, 'terminal subscriptions must be released before shutdown');
    if (batch < 64) f.nextHead();
  }
  assert.equal(f.releases, 256); assert.ok(f.peak <= 4);
  await f.worker.stop(); assert.equal(f.active, 0);
});

test('a submission deadline releases an unsubscribe promise that arrives late', async t => {
  let provideUnsubscribe, unsubscribe;
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = await fixture(t, { submit: details => {
    unsubscribe = details.unsubscribe;
    return new Promise(resolve => { provideUnsubscribe = resolve; });
  } });
  await until(() => f.calls === 1, 'submission starts');
  t.mock.timers.tick(20_001);
  await until(() => f.logs.some(message => message.includes('submission deadline')), 'deadline is reported');
  provideUnsubscribe(unsubscribe); await immediate();
  assert.equal(f.releases, 1, 'late subscription must be released after timeout');
});

test('a ready transaction with no terminal status has a bounded observation lifetime', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = await fixture(t, { submit: ({ callback, unsubscribe, accepted }) => {
    accepted(); callback({ status: { isReady: true } });
    return Promise.resolve(unsubscribe);
  } });
  await until(() => f.calls === 1, 'submission starts'); await immediate();
  assert.equal(f.active, 1, 'ready is not a terminal status');
  t.mock.timers.tick(20_001); await immediate();
  assert.equal(f.active, 1, 'accepted transactions outlive the submission deadline');
  t.mock.timers.tick(100_000); await immediate();
  assert.equal(f.active, 0, 'missing terminal status must not retain a subscription indefinitely');
  assert.equal(f.releases, 1);
});

for (const status of ['isInvalid', 'isDropped', 'isUsurped']) {
  test(`${status} releases a subscription even when rejection precedes its promise`, async t => {
    const f = await fixture(t, { submit: ({ callback, unsubscribe }) => {
      callback({ status: { [status]: true } });
      return Promise.resolve(unsubscribe);
    } });
    await until(() => f.logs.some(message => message.includes('not accepted')), 'terminal rejection is reported');
    await immediate();
    assert.equal(f.releases, 1); assert.equal(f.active, 0);
  });
}

test('shutdown interrupts a pending submission and releases its watcher exactly once', async t => {
  const f = await fixture(t, { submit: ({ unsubscribe }) => Promise.resolve(unsubscribe) });
  await until(() => f.calls === 1, 'submission starts'); await immediate();
  await f.worker.stop();
  assert.equal(f.releases, 1); assert.equal(f.active, 0);
  await f.worker.stop(); assert.equal(f.releases, 1);
});

test('a synchronous signing error cancels both submission and observation timers', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = await fixture(t, { submit: () => { throw new Error('Signing failed synchronously'); } });
  await until(() => f.logs.some(message => message.includes('Signing failed synchronously')), 'signing failure is reported');
  const count = f.logs.length;
  t.mock.timers.tick(120_001); await immediate();
  assert.equal(f.logs.length, count, 'failed signing leaves no observation timer running');
});
