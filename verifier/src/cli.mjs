import { parseArgs } from "node:util";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { ApiPromise, WsProvider, Keyring } from "@polkadot/api";
import { cryptoWaitReady } from "@polkadot/util-crypto";
import { startVerifier } from "./worker.mjs";
const { values } = parseArgs({
  options: {
    endpoint: { type: "string", default: "ws://127.0.0.1:9944" },
    state: { type: "string" },
    "key-file": { type: "string" },
    dev: { type: "boolean", default: false },
    fixtures: { type: "string" },
  },
});
let api, worker;
try {
  api = await ApiPromise.create({
    provider: new WsProvider(values.endpoint),
    noInitWarn: true,
  });
  const development = (await api.rpc.system.chainType()).isDevelopment;
  if ((values.dev || values.fixtures) && !development)
    throw new Error("Development verifier options require a development chain");
  if (!values.dev && !values["key-file"])
    throw new Error(
      "Supply --key-file or --dev for the local development verifier",
    );
  await cryptoWaitReady();
  let pair;
  try {
    pair = new Keyring({ type: "sr25519" }).addFromUri(
      values.dev
        ? "//Alice//ownr"
        : (await readFile(values["key-file"], "utf8")).trim(),
    );
  } catch {
    throw new Error("Unable to initialize verifier key");
  }
  const stateFile =
    values.state ??
    resolve(
      ".state",
      `${api.genesisHash.toHex().slice(2)}-${Buffer.from(pair.publicKey).toString("hex")}.json`,
    );
  worker = await startVerifier({
    api,
    pair,
    stateFile,
    fixtures: values.fixtures,
    log: console.error,
  });
  console.log(
    values.fixtures
      ? "Verifier ready (DEVELOPMENT FIXTURE MODE)"
      : "Verifier ready",
  );
  for (const signal of ["SIGINT", "SIGTERM"])
    process.once(signal, async () => {
      await worker.stop();
      await api.disconnect();
      process.exit(0);
    });
} catch (error) {
  console.error(error.message);
  await worker?.stop();
  await api?.disconnect();
  process.exitCode = 1;
}
