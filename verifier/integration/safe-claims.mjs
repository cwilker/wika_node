import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ApiPromise, WsProvider, Keyring } from "@polkadot/api";
import { cryptoWaitReady } from "@polkadot/util-crypto";
import { u8aToHex } from "@polkadot/util";
import { startVerifier } from "../src/worker.mjs";
// Exercise the real core encoder and wallet adapter boundary against the candidate runtime.
const { WikaClient, signClaimProof } = await import(
  process.env.WIKA_CORE_MODULE ?? "../../../wika_plugins_react/packages/core/dist/index.js"
);
assert.ok(
  process.env.WIKA_NODE_URL,
  "WIKA_NODE_URL must point at an isolated development chain",
);
const api = await ApiPromise.create({
  provider: new WsProvider(process.env.WIKA_NODE_URL),
  noInitWarn: true,
});
const directory = await mkdtemp(join(tmpdir(), "wika-safe-claims-live-"));
let worker;
const submit = async (tx, pair) =>
  new Promise((resolve, reject) => {
    let stop,
      done = false;
    const timer = setTimeout(() => {
      done = true;
      stop?.();
      reject(new Error("Transaction finality deadline"));
    }, 90000);
    tx.signAndSend(pair, { nonce: -1 }, (result) => {
      if (!result.status.isFinalized || done) return;
      done = true;
      clearTimeout(timer);
      stop?.();
      if (result.dispatchError) {
        const e = result.dispatchError;
        reject(
          new Error(
            e.isModule
              ? JSON.stringify(api.registry.findMetaError(e.asModule))
              : e.toString(),
          ),
        );
      } else resolve(result);
    }).then(
      (unsub) => {
        stop = unsub;
        if (done) unsub();
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
const wait = async (fn, label) => {
  const deadline = Date.now() + 120000;
  while (Date.now() < deadline) {
    if (await fn()) return;
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`Timed out: ${label}`);
};
try {
  assert.equal((await api.rpc.system.chainType()).isDevelopment, true);
  await cryptoWaitReady();
  const keys = new Keyring({ type: "sr25519" });
  const alice = keys.addFromUri("//Alice"),
    bob = keys.addFromUri("//Bob"),
    charlie = keys.addFromUri("//Charlie"),
    reviewer = keys.addFromUri("//Alice//ownr");
  assert.equal(api.runtimeVersion.specVersion.toNumber(), 402);
  // Short development windows keep this integration check bounded; product day windows stay intact.
  await submit(
    api.tx.sudo.sudo(
      api.tx.system.setStorage([
        [api.query.owners.numBlocksToCommit.key(), "0x02"],
        [api.query.owners.numBlocksToReveal.key(), "0x02"],
      ]),
    ),
    alice,
  );
  const page = `https://safe-claims.example/post-${Date.now()}`;
  const fixtures = join(directory, "fixtures.json");
  await writeFile(fixtures, "{}");
  worker = await startVerifier({
    api,
    pair: reviewer,
    stateFile: join(directory, "verifier.json"),
    fixtures,
    log: (message) => console.log(message),
  });
  const client = WikaClient.fromApi(api);
  const authorAmount = 330_000_000_000n;
  const initialLiability = (
    await api.query.owners.totalRewardLiability()
  ).toString();
  await submit(api.tx.likes.like(page + "#comments", "", 1), alice);
  assert.equal(
    (await api.query.owners.unclaimedRewards(page)).toString(),
    String(authorAmount),
  );
  const makeProof = async (pair) =>
    client.claims.createProof(
      { address: pair.address, source: "test" },
      {
        signRaw: async ({ data }) => ({
          id: 1,
          signature: u8aToHex(pair.sign(Buffer.from(data.slice(2), "hex"))),
        }),
      },
      page,
    );
  const publish = async (proof) =>
    writeFile(
      fixtures,
      JSON.stringify({
        [page]: `<!doctype html><html><head><meta name="wika-claim" content="${proof.content}"></head><body>A visitor's comment is not proof.</body></html>`,
      }),
    );
  const first = await makeProof(bob);
  await publish(first);
  const firstQuote = await client.claims.quote(bob.address, {
    kind: "claim",
    proof: first,
  });
  assert.equal(firstQuote.verificationFee, 5_000_000_000_000n);
  assert.equal(firstQuote.refundableBond, 0n);
  await submit(api.tx.owners.claimPage(first.content), bob);
  await wait(
    async () => !(await api.query.owners.currentPeriod(page)).isNone,
    "initial owner",
  );
  assert.equal(
    (await api.query.owners.periods(page, 1)).unwrap().balance.toString(),
    String(authorAmount),
  );
  assert.equal((await api.query.owners.unclaimedRewards(page)).toString(), "0");
  assert.equal(
    (await client.claims.getPage(page)).current.balance,
    authorAmount,
  );
  console.log("Initial claim and funded backlog verified");
  // Restart with the durable state file; the next request still observes the accepted persistent tag.
  await worker.stop();
  worker = await startVerifier({
    api,
    pair: reviewer,
    stateFile: join(directory, "verifier.json"),
    fixtures,
    log: (message) => console.log(message),
  });
  await submit(api.tx.owners.verifyWithdrawal(page, 1), bob);
  await wait(async () => {
    const p = (await api.query.owners.periods(page, 1)).unwrap();
    return (
      p.verifiedAt.isSome && (await api.query.owners.claimChecks(page)).isNone
    );
  }, "fresh withdrawal verification");
  console.log("Persistent proof refresh verified");
  const replacement = await makeProof(charlie);
  await publish(replacement);
  const replacementQuote = await client.claims.quote(charlie.address, {
    kind: "claim",
    proof: replacement,
  });
  assert.equal(replacementQuote.refundableBond, 1_000_000_000_000n);
  await submit(api.tx.owners.claimPage(replacement.content), charlie);
  await wait(
    async () => (await api.query.owners.pendingReplacements(page)).isSome,
    "replacement window",
  );
  assert.equal(
    (await api.query.owners.currentPeriod(page)).unwrap().toString(),
    "1",
  );
  const pending = (await api.query.owners.pendingReplacements(page)).unwrap();
  await submit(api.tx.owners.approveReplacement(page, pending.id), bob);
  await submit(api.tx.owners.approveReplacement(page, pending.id), charlie);
  await submit(api.tx.owners.finalizeReplacement(page, pending.id), charlie);
  await wait(
    async () =>
      (await api.query.owners.currentPeriod(page)).unwrap().toString() === "2",
    "cooperative activation with fresh quorum",
  );
  assert.equal(
    (await api.query.owners.periods(page, 1)).unwrap().balance.toString(),
    String(authorAmount),
  );
  assert.equal(
    (await api.query.owners.periods(page, 2)).unwrap().balance.toString(),
    "0",
  );
  // Bob no longer controls the page. His historical earnings still require only his wallet.
  await submit(api.tx.owners.withdrawRewards(page, 1, authorAmount), bob);
  assert.equal(
    (await api.query.owners.totalRewardLiability()).toString(),
    initialLiability,
  );
  assert.equal(
    (await api.query.owners.periods(page, 1)).unwrap().balance.toString(),
    "0",
  );
  assert.equal((await api.query.owners.pendingReplacements(page)).isNone, true);
  assert.equal((await api.query.owners.activeChecksTotal()).toString(), "0");
  console.log(
    "PASS: funded backlog, signed claim, durable verifier, refresh, cooperative transfer, historical withdrawal",
  );
} finally {
  await worker?.stop();
  await api.disconnect();
  await rm(directory, { recursive: true, force: true });
}
