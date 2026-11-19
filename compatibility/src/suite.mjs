import assert from "node:assert/strict";
import { readFile, mkdir, writeFile, access } from "node:fs/promises";
import { join, resolve } from "node:path";
import { createRequire } from "node:module";
import { command } from "./process.mjs";
import { connect, checkFiles, harnessRoot } from "./cli.mjs";

/** Entry point shared by consumer-owned test:compatibility commands. */
export async function runSuite(root) {
  const suite = JSON.parse(
    await readFile(join(root, "compatibility/suite.json"), "utf8"),
  );
  if (suite.schemaVersion !== 1 || !suite.consumer || !suite.steps?.length)
    throw new Error("Invalid compatibility suite");
  if (
    process.env.WIKA_CONSUMER_ID &&
    process.env.WIKA_CONSUMER_ID !== suite.consumer
  )
    throw new Error("Checked-out consumer does not match the CI matrix");
  const endpoint = process.env.WIKA_NODE_URL;
  if (!endpoint)
    throw new Error(
      "WIKA_NODE_URL is required; compatibility tests never silently skip.",
    );
  for (const name of suite.requiredEnv ?? [])
    if (!process.env[name])
      throw new Error(
        `${name} is required by ${suite.consumer}; compatibility tests never silently skip.`,
      );
  const reportDirectory = resolve(
    process.env.WIKA_COMPAT_REPORT_DIR ?? join(root, "compatibility/reports"),
  );
  await mkdir(reportDirectory, { recursive: true });
  await writeFile(join(reportDirectory, "suite.log"), "");
  const report = {
    schemaVersion: 1,
    consumer: suite.consumer,
    passed: false,
    startedAt: new Date().toISOString(),
    stages: [],
  };
  try {
    if (suite.consumer === "plugin") {
      const stage = { name: "Shared claim identity/proof fixtures", passed: false };
      report.stages.push(stage);
      for (const name of ["page-identity-v1.json", "claim-proof-v1.json", "host-referral-v1.json", "economic-terms-v2.json"]) {
        const expected = JSON.parse(await readFile(join(harnessRoot, "fixtures", name), "utf8"));
        const actual = JSON.parse(await readFile(join(root, "packages/core/tests/fixtures", name), "utf8"));
        assert.deepEqual(actual, expected, `Shared fixture mismatch: ${name}; update the core library and tests with the node.`);
      }
      stage.passed = true;
    }
    if (suite.consumer === "browser") {
      const stage = { name: "Shared page identity fixtures", passed: false };
      report.stages.push(stage);
      for (const name of ["page-identity-v1.json", "claim-proof-v1.json", "host-referral-v1.json", "economic-terms-v2.json"]) {
        const expected = JSON.parse(await readFile(join(harnessRoot, "fixtures", name), "utf8"));
        const actual = JSON.parse(await readFile(join(root, "v2/tests/fixtures", name), "utf8"));
        assert.deepEqual(actual, expected, `Shared fixture mismatch: ${name}; update the shipped extension core with the node.`);
      }
      stage.passed=true;
    }
    const api = await connect(endpoint);
    try {
      if (!(await api.rpc.system.chainType()).isDevelopment)
        throw new Error(
          "Compatibility behavior tests require an isolated development chain; never run them against a public network.",
        );
      const files = [
        join(root, "compatibility/node-contract.json"),
        join(harnessRoot, `supported/${suite.consumer}.json`),
      ];
      if (process.env.WIKA_PREVIOUS_CONTRACTS) {
        const old = join(
          process.env.WIKA_PREVIOUS_CONTRACTS,
          `${suite.consumer}.json`,
        );
        try {
          await access(old);
          files.push(old);
        } catch (error) {
          if (error.code !== "ENOENT") throw error;
          console.log(
            "No prior contract for this consumer at the base commit (initial rollout).",
          );
        }
      }
      const metadata = await checkFiles(
        api,
        files,
        join(reportDirectory, "metadata.json"),
      );
      report.runtime = metadata.runtime;
      report.stages.push({ name: "Node contract", passed: metadata.passed });
      if (!metadata.passed)
        throw new Error(
          `${suite.consumer} is incompatible with this node; see metadata.json for the exact requirements.`,
        );
    } finally {
      await api.disconnect();
    }
    const cwd = resolve(root, suite.packageDirectory);
    if (suite.consumer === "etl") {
      // A dedicated test database; SubQuery historical storage requires this extension.
      const require = createRequire(join(cwd, "package.json"));
      const { Client } = require("pg");
      const db = new Client({
        host: process.env.WIKA_TEST_DB_HOST ?? "127.0.0.1",
        port: Number(process.env.WIKA_TEST_DB_PORT),
        user: process.env.WIKA_TEST_DB_USER ?? "postgres",
        password: process.env.WIKA_TEST_DB_PASS ?? "postgres",
        database: process.env.WIKA_TEST_DB_NAME ?? "postgres",
      });
      try {
        await db.connect();
        await db.query("CREATE EXTENSION IF NOT EXISTS btree_gist");
      } finally {
        await db.end();
      }
    }
    for (const step of suite.steps) {
      if (!step.name || !Array.isArray(step.command) || !step.command.length)
        throw new Error("Invalid suite step");
      console.log(`\n[${suite.consumer}] ${step.name}`);
      const stage = { name: step.name, passed: false };
      report.stages.push(stage);
      await command(step.command, {
        cwd,
        log: join(reportDirectory, "suite.log"),
        timeoutMs: step.timeoutMs ?? 360000,
      });
      stage.passed = true;
    }
    report.passed = true;
  } catch (error) {
    report.error = error.message;
    throw error;
  } finally {
    report.finishedAt = new Date().toISOString();
    await writeFile(
      join(reportDirectory, "result.json"),
      JSON.stringify(report, null, 2) + "\n",
    );
  }
}
