import { spawn } from "node:child_process";
import { createWriteStream } from "node:fs";
import { access, mkdir, readFile, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { parseArgs } from "node:util";
import { resolve, join } from "node:path";
import { command, stopProcess } from "./process.mjs";
import { harnessRoot } from "./cli.mjs";

async function freePort() {
  const server = createServer();
  await new Promise((resolve, reject) =>
    server.once("error", reject).listen(0, "127.0.0.1", resolve),
  );
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return port;
}
async function run(consumer, root, binary, reportDirectory) {
  const contract = JSON.parse(
    await readFile(join(root, "compatibility/node-contract.json"), "utf8"),
  );
  if (contract.consumer !== consumer.id)
    throw new Error(`Expected ${consumer.id}, received ${contract.consumer}`);
  if (consumer.id === "etl" && !process.env.WIKA_TEST_DB_PORT)
    throw new Error(
      "Set WIKA_TEST_DB_PORT for a dedicated test Postgres before running the ETL suite.",
    );
  await mkdir(reportDirectory, { recursive: true });
  const rpcPort = await freePort(),
    networkPort = await freePort();
  const endpoint = `ws://127.0.0.1:${rpcPort}`;
  const log = createWriteStream(join(reportDirectory, "node.log"));
  const child = spawn(
    binary,
    [
      "--dev",
      "--tmp",
      `--rpc-port=${rpcPort}`,
      `--port=${networkPort}`,
      "--no-prometheus",
    ],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  let spawnError;
  child.on("error", (error) => {
    spawnError = error;
  });
  child.stdout.pipe(log, { end: false });
  child.stderr.pipe(log, { end: false });
  let interrupted = false;
  const interrupt = () => {
    interrupted = true;
    child.kill("SIGTERM");
  };
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", interrupt);
  try {
    const deadline = Date.now() + 60000;
    let ready = false;
    while (Date.now() < deadline) {
      if (spawnError) throw spawnError;
      if (interrupted || child.exitCode !== null || child.signalCode !== null)
        throw new Error(
          "Candidate node exited before its RPC was ready; see node.log",
        );
      try {
        const response = await fetch(`http://127.0.0.1:${rpcPort}`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            jsonrpc: "2.0",
            id: 1,
            method: "chain_getHeader",
          }),
          signal: AbortSignal.timeout(2000),
        });
        const result = await response.json();
        if (result.result && BigInt(result.result.number) > 0n) {
          ready = true;
          break;
        }
      } catch {
        /* Wait for startup only; readiness itself has a deadline. */
      }
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
    if (!ready)
      throw new Error(
        "Candidate node RPC did not become ready in 60 seconds; see node.log",
      );
    const env = {
      ...process.env,
      WIKA_NODE_URL: endpoint,
      WIKA_NODE_CHECKOUT: resolve(harnessRoot, ".."),
      WIKA_CONSUMER_ID: consumer.id,
      WIKA_COMPAT_REPORT_DIR: reportDirectory,
    };
    const cwd = resolve(root, consumer.packageDirectory);
    await command(["npm", "run", "test:compatibility"], {
      cwd,
      env,
      log: join(reportDirectory, "consumer.log"),
      timeoutMs: 720000,
    });
  } finally {
    await stopProcess(child);
    log.end();
    process.removeListener("SIGINT", interrupt);
    process.removeListener("SIGTERM", interrupt);
  }
}
async function main() {
  const { values } = parseArgs({
    options: {
      consumer: { type: "string" },
      "consumer-root": { type: "string" },
      workspace: { type: "string", default: "../.." },
      "node-binary": { type: "string", default: "../target/release/wika-node" },
      "report-directory": { type: "string", default: "reports" },
      all: { type: "boolean", default: false },
    },
  });
  const manifest = JSON.parse(
    await readFile(join(harnessRoot, "consumers.json"), "utf8"),
  );
  const selected = values.all
    ? manifest.consumers
    : manifest.consumers.filter((c) => c.id === values.consumer);
  if (!selected.length)
    throw new Error("Choose --consumer plugin|etl|browser or --all");
  if (values.all && values["consumer-root"])
    throw new Error("--consumer-root requires exactly one consumer");
  const binary = resolve(values["node-binary"]);
  await access(binary);
  const results = [];
  for (const consumer of selected) {
    const root = values["consumer-root"]
      ? resolve(values["consumer-root"])
      : resolve(values.workspace, consumer.directory);
    try {
      await run(
        consumer,
        root,
        binary,
        resolve(values["report-directory"], consumer.id),
      );
      results.push({ consumer: consumer.id, passed: true });
    } catch (error) {
      console.error(`FAIL ${consumer.id}: ${error.message}`);
      results.push({
        consumer: consumer.id,
        passed: false,
        error: error.message,
      });
    }
  }
  await mkdir(resolve(values["report-directory"]), { recursive: true });
  await writeFile(
    resolve(values["report-directory"], "summary.json"),
    JSON.stringify(results, null, 2) + "\n",
  );
  if (results.some((r) => !r.passed)) process.exitCode = 1;
}
main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
