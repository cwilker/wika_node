import { readFile, readdir, appendFile } from "node:fs/promises";
import { join, resolve } from "node:path";
const root = resolve(process.argv[2] ?? "reports");
const lines = [
  "## Node consumer compatibility",
  "",
  "| Consumer | Result |",
  "| --- | --- |",
];
let found = false;
for (const directory of await readdir(root, { withFileTypes: true }).catch(
  () => [],
)) {
  if (!directory.isDirectory()) continue;
  const base = join(root, directory.name);
  try {
    const report = JSON.parse(
      await readFile(join(base, "result.json"), "utf8"),
    );
    found = true;
    lines.push(`| ${report.consumer} | ${report.passed ? "PASS" : "FAIL"} |`);
    if (!report.passed) {
      lines.push(
        "",
        `### ${report.consumer}`,
        "",
        "```text",
        report.error ?? "Failed; inspect the attached logs.",
      );
      const metadata = await readFile(join(base, "metadata.json"), "utf8")
        .then(JSON.parse)
        .catch(() => null);
      for (const result of metadata?.results ?? [])
        for (const failure of result.failures) lines.push(failure);
      lines.push("```", "");
    }
  } catch {
    /* Startup/checkout failures still have workflow logs. */
  }
}
if (!found)
  lines.push("| No completed report | Setup failed; inspect the job logs. |");
const summary = lines.join("\n") + "\n";
console.log(summary);
if (process.env.GITHUB_STEP_SUMMARY)
  await appendFile(process.env.GITHUB_STEP_SUMMARY, summary);
