import { ApiPromise, WsProvider } from "@polkadot/api";
import { parseArgs } from "node:util";
import { readFile, readdir, mkdir, writeFile } from "node:fs/promises";
import { dirname, resolve, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";
import { checkContract, validateContract } from "./contract.mjs";
import { readSurface } from "./surface.mjs";

export const harnessRoot = fileURLToPath(new URL("..", import.meta.url));
export async function connect(endpoint) {
  const provider = new WsProvider(endpoint);
  const api = new ApiPromise({ provider, noInitWarn: true });
  let timer;
  try {
    await Promise.race([
      api.isReadyOrError,
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(new Error(`Node connection timed out: ${endpoint}`)),
          20000,
        );
      }),
    ]);
    return api;
  } catch (error) {
    await provider.disconnect();
    throw error;
  } finally {
    clearTimeout(timer);
  }
}
export async function checkFiles(api, files, reportPath) {
  if (!files.length) throw new Error("No consumer contracts selected");
  const loaded = await Promise.all(
    files.map(async (file) => ({
      file,
      contract: validateContract(JSON.parse(await readFile(file, "utf8"))),
    })),
  );
  const requirements = {};
  for (const { contract } of loaded)
    for (const category of [
      "calls",
      "events",
      "storage",
      "constants",
      "errors",
    ]) {
      requirements[category] = {
        ...requirements[category],
        ...contract.requirements[category],
      };
    }
  requirements.exhaustiveEvents = [...new Set(loaded.flatMap(({contract}) => contract.requirements.exhaustiveEvents ?? []))];
  const surface = await readSurface(api, requirements);
  const results = [];
  for (const { file, contract } of loaded) {
    const result = { ...checkContract(contract, surface), contract: file };
    results.push(result);
    console.log(
      `${result.passed ? "PASS" : "FAIL"} ${contract.consumer}: ${file}`,
    );
    for (const failure of result.failures) console.error(`  ${failure}`);
  }
  const report = {
    schemaVersion: 1,
    generatedAt: new Date().toISOString(),
    runtime: {
      specName: api.runtimeVersion.specName.toString(),
      specVersion: api.runtimeVersion.specVersion.toNumber(),
      transactionVersion: api.runtimeVersion.transactionVersion.toNumber(),
      genesisHash: api.genesisHash.toHex(),
      metadataSha256: createHash("sha256")
        .update(api.runtimeMetadata.toU8a())
        .digest("hex"),
    },
    passed: results.every((r) => r.passed),
    results,
  };
  if (reportPath) {
    await mkdir(dirname(reportPath), { recursive: true });
    await writeFile(reportPath, JSON.stringify(report, null, 2) + "\n");
  }
  return report;
}
export async function contractFiles(directory) {
  return (await readdir(directory))
    .filter((name) => name.endsWith(".json"))
    .sort()
    .map((name) => join(directory, name));
}
async function main() {
  const { values } = parseArgs({
    options: {
      endpoint: { type: "string" },
      contract: { type: "string", multiple: true },
      workspace: { type: "string", default: "../.." },
      "baseline-directory": { type: "string" },
      report: { type: "string", default: "reports/contracts.json" },
    },
  });
  const endpoint = values.endpoint ?? process.env.WIKA_NODE_URL;
  if (!endpoint)
    throw new Error(
      "Set WIKA_NODE_URL or pass --endpoint. This check never silently skips.",
    );
  let files = values.contract?.map((file) => resolve(file));
  if (!files?.length) {
    const manifest = JSON.parse(
      await readFile(join(harnessRoot, "consumers.json"), "utf8"),
    );
    files = manifest.consumers.map((c) =>
      resolve(
        values.workspace,
        c.directory,
        "compatibility/node-contract.json",
      ),
    );
  }
  files.push(
    ...(await contractFiles(
      values["baseline-directory"]
        ? resolve(values["baseline-directory"])
        : join(harnessRoot, "supported"),
    )),
  );
  const api = await connect(endpoint);
  try {
    if (!(await checkFiles(api, files, resolve(values.report))).passed)
      process.exitCode = 1;
  } finally {
    await api.disconnect();
  }
}
if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
)
  main().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
