import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, readFile, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { startVerifier } from "../src/worker.mjs";
function api(genesis = "0x" + "01".repeat(32)) {
  const header = { number: { toNumber: () => 1 }, hash: "0xblock" };
  const view = {
    query: {
      timestamp: { now: async () => 1000 },
      owners: { requests: { entries: async () => [] } },
    },
  };
  return {
    genesisHash: { toHex: () => genesis },
    query: {
      owners: {
        claimChecks: () => {},
        verifiers: async () => ({
          isNone: false,
          unwrap: () => [null, { isTrue: true }],
        }),
      },
    },
    rpc: {
      system: { chainType: async () => ({ isDevelopment: true }) },
      chain: {
        subscribeNewHeads: async () => () => {},
        getHeader: async () => header,
      },
    },
    at: async () => view,
  };
}
test("verifier persists private state, excludes a second writer, and restarts on the same chain", async () => {
  const directory = await mkdtemp(join(tmpdir(), "wika-verifier-state-test-")),
    stateFile = join(directory, "state.json");
  const pair = {
    address: "development-verifier",
    publicKey: new Uint8Array(32),
  };
  let worker;
  try {
    worker = await startVerifier({ api: api(), pair, stateFile });
    await assert.rejects(
      startVerifier({ api: api(), pair, stateFile }),
      /locked/,
    );
    await worker.stop();
    worker = null;
    assert.equal((await stat(stateFile)).mode & 0o077, 0);
    const saved = JSON.parse(await readFile(stateFile, "utf8"));
    assert.equal(saved.version, 1);
    assert.deepEqual(saved.items, {});
    await assert.rejects(stat(stateFile + ".lock"), { code: "ENOENT" });
    await assert.rejects(
      startVerifier({ api: api("0x" + "02".repeat(32)), pair, stateFile }),
      /different chain/,
    );
    worker = await startVerifier({ api: api(), pair, stateFile });
    await worker.stop();
    worker = null;
  } finally {
    await worker?.stop();
    await rm(directory, { recursive: true, force: true });
  }
});
