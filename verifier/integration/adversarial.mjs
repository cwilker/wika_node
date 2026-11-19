import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createWriteStream } from 'node:fs';
import { mkdir, mkdtemp, readFile, writeFile, rename, rm, unlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createServer } from 'node:net';
import { parseArgs } from 'node:util';
import { createHash } from 'node:crypto';
import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';
import { cryptoWaitReady } from '@polkadot/util-crypto';
import { fixtureProof } from './fixture-proof.mjs';
import { codecBytes } from '../src/ballot.mjs';
import { stopProcess } from '../../compatibility/src/process.mjs';

// Always owns a fresh temporary node. Never changes a supplied/operator chain.
const root = fileURLToPath(new URL('../..', import.meta.url));
const { values } = parseArgs({ options: {
  'node-binary': { type: 'string', default: join(root, 'target/release/wika-node') },
  'report-directory': { type: 'string', default: join(root, 'compatibility/reports', `verifier-adversarial-${Date.now()}`) },
} });
const reportDirectory = resolve(values['report-directory']);
await mkdir(reportDirectory, { recursive: true });
const directory = await mkdtemp(join(tmpdir(), 'wika-adversarial-'));
const report = { schemaVersion: 1, passed: false, startedAt: new Date().toISOString(), scenarios: [], sources: {} };
const processes = new Set();
let api, node, interrupted = false;
const interrupt = () => { interrupted = true; };
process.once('SIGINT', interrupt);
process.once('SIGTERM', interrupt);

async function wait(fn, label, timeout = 120000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (interrupted) throw new Error('Integration interrupted');
    if (node && (node.child.exitCode !== null || node.child.signalCode !== null || node.error))
      throw new Error('Owned development node exited; see node.log');
    const value = await fn();
    if (value) return value;
    await new Promise(r => setTimeout(r, 200));
  }
  throw new Error(`Timed out: ${label}`);
}
async function port() {
  const server = createServer();
  await new Promise((yes, no) => server.once('error', no).listen(0, '127.0.0.1', yes));
  const result = server.address().port;
  await new Promise(yes => server.close(yes));
  return result;
}
function launch(binary, args, name) {
  const output = createWriteStream(join(reportDirectory, name + '.log'), { flags: 'a' });
  const child = spawn(binary, args, { stdio: ['ignore', 'pipe', 'pipe'] });
  const process = { child, output, text: '' };
  processes.add(process);
  for (const stream of [child.stdout, child.stderr]) stream.on('data', data => {
    output.write(data);
    process.text = (process.text + data).slice(-20000);
  });
  child.on('error', error => { process.error = error; });
  return process;
}
async function stop(process, crash = false) {
  if (!process) return;
  if (crash && process.child.exitCode === null && process.child.signalCode === null) {
    await new Promise(yes => { process.child.once('exit', yes); process.child.kill('SIGKILL'); });
  } else await stopProcess(process.child);
  await new Promise(yes => process.output.end(yes));
  processes.delete(process);
}
async function submit(tx, pair, expectedError) {
  return new Promise((yes, no) => {
    let unsub, done = false;
    const finish = (error, result) => {
      if (done) return;
      done = true; clearTimeout(timer); unsub?.();
      error ? no(error) : yes(result);
    };
    const timer = setTimeout(() => finish(new Error('Transaction finality timeout')), 90000);
    tx.signAndSend(pair, { nonce: -1 }, result => {
      if (!result.status.isFinalized) return;
      let error = result.dispatchError;
      const sudo = result.events.find(({ event }) => event.section === 'sudo' && event.method === 'Sudid');
      if (sudo?.event.data[0].isErr) error = sudo.event.data[0].asErr;
      const name = error?.isModule ? api.registry.findMetaError(error.asModule).name : error?.toString();
      if (expectedError) {
        finish(name === expectedError ? undefined : new Error(`Expected ${expectedError}, received ${name ?? 'success'}`), result);
      } else finish(error ? new Error(name) : undefined, result);
    }).then(stop => { unsub = stop; if (done) stop(); }, finish);
  });
}

try {
  for (const path of [values['node-binary'], join(root, 'verifier/integration/adversarial.mjs'),
    join(root, 'verifier/src/worker.mjs'), join(root, 'verifier/src/ballot.mjs'),
    join(root, 'verifier/src/proof.mjs'), join(root, 'verifier/src/proof-html.mjs'),
    join(root, 'verifier/integration/fixture-proof.mjs'), join(root, 'verifier/src/cli.mjs'),
    join(root, 'compatibility/src/process.mjs'), join(root, 'verifier/package-lock.json')])
    report.sources[resolve(path)] = createHash('sha256').update(await readFile(path)).digest('hex');
  const rpc = await port(), network = await port();
  const endpoint = `ws://127.0.0.1:${rpc}`;
  node = launch(resolve(values['node-binary']), ['--dev', '--tmp', `--rpc-port=${rpc}`, `--port=${network}`,
    '--no-prometheus', '--no-telemetry', '--offchain-worker=never'], 'node');
  await wait(async () => {
    try {
      const res = await fetch(`http://127.0.0.1:${rpc}`, { method: 'POST', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'chain_getHeader' }), signal: AbortSignal.timeout(1000) });
      return BigInt((await res.json()).result?.number ?? 0) > 0n;
    } catch { return false; }
  }, 'development RPC', 60000);
  api = await ApiPromise.create({ provider: new WsProvider(endpoint), noInitWarn: true });
  assert.equal((await api.rpc.system.chainType()).isDevelopment, true);
  assert.equal(api.runtimeVersion.specVersion.toNumber(), 402);
  report.runtime = { specVersion: 402, genesis: api.genesisHash.toHex(), node: (await api.rpc.system.version()).toString() };
  await cryptoWaitReady();
  const keys = new Keyring({ type: 'sr25519' });
  const alice = keys.addFromUri('//Alice'), bob = keys.addFromUri('//Bob');
  const reviewers = ['Alice', 'Bob', 'Charlie', 'Dave'].map(name => keys.addFromUri(`//${name}//ownr`));
  for (const reviewer of reviewers.slice(1)) {
    await submit(api.tx.balances.transferKeepAlive(reviewer.address, '1000000000000000'), alice);
    await submit(api.tx.sudo.sudo(api.tx.owners.addVerifier(reviewer.address)), alice);
  }
  const storage = [
    [api.query.owners.numBlocksToCommit.key(), '0x04'],
    [api.query.owners.numBlocksToReveal.key(), '0x04'],
    [api.query.owners.prctNeededForAgreement.key(), '0x32'],
    [api.query.owners.majorityMin.key(), '0x03000000'],
  ];
  await submit(api.tx.sudo.sudo(api.tx.system.setStorage(storage)), alice);
  assert.equal((await api.query.owners.verifiers.entries()).length, 4);
  const maps = reviewers.map(() => ({})), workers = [];
  const fixtures = i => join(directory, `fixtures-${i}.json`);
  const state = i => join(directory, `state-${i}.json`);
  async function publish() {
    for (let i = 0; i < 4; i++) {
      await writeFile(fixtures(i) + '.tmp', JSON.stringify(maps[i]));
      await rename(fixtures(i) + '.tmp', fixtures(i));
    }
  }
  await publish();
  async function start(i) {
    const key = join(directory, `key-${i}`);
    await writeFile(key, `//${['Alice', 'Bob', 'Charlie', 'Dave'][i]}//ownr`, { mode: 0o600 });
    const worker = launch(process.execPath, [join(root, 'verifier/src/cli.mjs'), '--endpoint', endpoint,
      '--key-file', key, '--state', state(i), '--fixtures', fixtures(i)], `worker-${i}`);
    workers[i] = worker;
    await wait(() => {
      if (worker.error || worker.child.exitCode !== null || worker.child.signalCode !== null)
        throw new Error(`Worker ${i} failed: ${worker.text}`);
      return worker.text.includes('Verifier ready');
    }, `worker ${i} startup`, 20000);
  }
  for (let i = 0; i < 4; i++) await start(i);
  report.workers = reviewers.map((key, i) => ({ address: key.address, independentProcess: true, stateFile: `state-${i}.json` }));

  let sequence = 0;
  async function setup(name, observations, owner = bob) {
    const page = `https://adversarial.example/${++sequence}-${name}`;
    const proof = fixtureProof(owner, api, page, BigInt((await api.query.timestamp.now()).toString()) + 3600000n);
    for (let i = 0; i < 4; i++) {
      if (observations[i] === true) maps[i][page] = `<html><head><title>Independent observer ${i}</title><meta name="wika-claim" content="${proof.content}"></head><body>${i}</body></html>`;
      else if (observations[i] === false) maps[i][page] = '<html><head></head><body>No publishing proof</body></html>';
      // Missing fixture is unavailable evidence: the worker abstains.
    }
    await publish();
    await submit(api.tx.likes.like(page, '', 1), alice);
    await submit(api.tx.owners.claimPage(proof.content), owner);
    const round = (await api.query.owners.rounds(page)).unwrap();
    assert.equal(round.members.length, 4);
    assert.equal(round.threshold.toNumber(), 3);
    return { name, page, owner, observations, commitEnd: round.commitEnd.toNumber(), revealEnd: round.revealEnd.toNumber() };
  }
  async function result(scenario, expected, matching, reveals) {
    const finished = await wait(async () => {
      const hash = await api.rpc.chain.getFinalizedHead();
      const view = await api.at(hash);
      const value = await view.query.owners.lastClaimResults(scenario.page);
      return value.isSome && (await view.query.owners.claimChecks(scenario.page)).isNone ? { view, value: value.unwrap() } : false;
    }, `${scenario.name} final outcome`);
    assert.equal(finished.value.outcome.toString(), expected);
    assert.equal((await finished.view.query.owners.currentPeriod(scenario.page)).isSome, expected === 'Approved');
    assert.equal((await finished.view.query.owners.unclaimedRewards(scenario.page)).toString(), expected === 'Approved' ? '0' : '330000000000');
    assert.equal((await finished.view.query.owners.retryCredits(scenario.page)).isSome, expected === 'Inconclusive');
    const tallies = [], submitted = new Set();
    for (let height = scenario.commitEnd - 3; height <= scenario.revealEnd + 1; height++) {
      const view = await api.at(await api.rpc.chain.getBlockHash(height));
      for (const { event } of await view.query.system.events()) {
        if (event.section !== 'owners') continue;
        if (event.method === 'VerificationFinished' && codecBytes(event.data[0]).toString() === scenario.page)
          tallies.push(event.data.toJSON());
        if (event.method === 'UrlCheckRevealed' && codecBytes(event.data[1]).toString() === scenario.page)
          submitted.add(event.data[0].toString());
      }
    }
    assert.equal(tallies.length, 1);
    const tally = tallies[0];
    assert.equal(tally[2], 4); assert.equal(tally[3], 3); assert.equal(tally[4], matching);
    assert.equal(submitted.size, reveals);
    const entry = { name: scenario.name, page: scenario.page, passed: true, outcome: expected,
      electorate: 4, threshold: 3, matching, reveals, observations: scenario.observations, ...scenario.evidence };
    report.scenarios.push(entry);
    console.log(`PASS ${entry.name}: ${expected}, ${matching}/4 matching, ${reveals} reveals`);
  }
  // Each round has four real OS processes, API connections and durable state files.
  for (const [name, observations, expected, matching, reveals] of [
    ['three-yes-one-no', [true, true, true, false], 'Approved', 3, 4],
    ['one-yes-three-no', [true, false, false, false], 'InvalidProof', 3, 4],
    ['two-yes-two-no', [true, true, false, false], 'Inconclusive', 2, 4],
    ['two-yes-two-unavailable', [true, true, null, null], 'Inconclusive', 2, 2],
    ['three-yes-one-unavailable', [true, true, true, null], 'Approved', 3, 3],
    ['one-yes-three-unavailable', [true, null, null, null], 'Inconclusive', 1, 1],
  ]) await result(await setup(name, observations), expected, matching, reveals);

  const recovery = await setup('crash-after-commit', [true, true, true, null]);
  await wait(async () => (await api.query.owners.commits(recovery.page, reviewers[2].address)).isSome, 'durable commitment');
  const committed = (await api.query.owners.commits(recovery.page, reviewers[2].address)).unwrap().toHex().slice(2);
  await stop(workers[2], true);
  const persisted = JSON.parse(await readFile(state(2), 'utf8'));
  assert.ok(Object.values(persisted.items).some(item => item.hash === committed), 'Ballot persisted before broadcast');
  // stop() has observed OS process exit before its confirmed stale lock is removed.
  await unlink(state(2) + '.lock');
  await start(2);
  recovery.evidence = { killedAfterCommit: true, recoveredCommitment: committed, sameStateFile: true };
  await result(recovery, 'Approved', 3, 3);

  const revoked = await setup('revoke-after-commit', [true, true, true, false]);
  await wait(async () => (await api.query.owners.commits(revoked.page, reviewers[2].address)).isSome, 'revoked key commitment');
  await submit(api.tx.sudo.sudo(api.tx.owners.disableVerifier(reviewers[2].address)), alice);
  revoked.evidence = { revokedAddress: reviewers[2].address, denominatorRemainsFour: true };
  await result(revoked, 'Inconclusive', 2, 3);
  await submit(api.tx.sudo.sudo(api.tx.owners.enableVerifier(reviewers[2].address)), alice);
  for (const worker of workers) await stop(worker);
  assert.equal((await api.query.owners.activeChecksTotal()).toString(), '0');
  // Every page's funded reward stays accounted for, including rejected/inconclusive requests.
  assert.equal((await api.query.owners.totalRewardLiability()).toString(), String(BigInt(sequence) * 330000000000n));
  report.passed = true;
} catch (error) {
  report.error = error.stack;
  console.error(error);
  process.exitCode = 1;
} finally {
  for (const child of [...processes].reverse()) await stop(child);
  await api?.disconnect();
  await rm(directory, { recursive: true, force: true });
  report.finishedAt = new Date().toISOString();
  report.cleanup = 'Owned node/workers stopped; temporary fixture/state/key files removed. No operator chain used.';
  await writeFile(join(reportDirectory, 'result.json'), JSON.stringify(report, null, 2) + '\n');
  process.removeListener('SIGINT', interrupt);
  process.removeListener('SIGTERM', interrupt);
  console.log(`Report: ${join(reportDirectory, 'result.json')}`);
}
