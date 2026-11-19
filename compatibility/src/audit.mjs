import { spawn } from 'node:child_process';
import { parseArgs } from 'node:util';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { harnessRoot } from './cli.mjs';

/** A separate release gate: functional compatibility never makes security findings green. */
const { values } = parseArgs({
  options: {
    workspace: { type: 'string', default: '../..' },
    consumer: { type: 'string' },
    'consumer-root': { type: 'string' },
    report: { type: 'string', default: 'reports/dependency-audit.json' },
  },
});
const manifest = JSON.parse(await readFile(join(harnessRoot, 'consumers.json'), 'utf8'));
const consumers = manifest.consumers.filter((item) => !values.consumer || item.id === values.consumer);
if (!consumers.length || (values['consumer-root'] && consumers.length !== 1))
  throw new Error('Choose one valid consumer when using --consumer-root');
const results = [];
for (const consumer of consumers) {
  const root = values['consumer-root']
    ? resolve(values['consumer-root'])
    : resolve(values.workspace, consumer.directory);
  const cwd = resolve(root, consumer.packageDirectory);
  const result = await new Promise((resolveResult) => {
    const child = spawn('npm', ['audit', '--json', '--audit-level=high'], { cwd, stdio: ['ignore', 'pipe', 'pipe'] });
    let output = '',
      diagnostic = '',
      finished = false;
    const timer = setTimeout(() => child.kill('SIGTERM'), 180000);
    const finish = (value) => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      resolveResult(value);
    };
    child.stdout.on('data', (data) => {
      output += data;
    });
    child.stderr.on('data', (data) => {
      diagnostic += data;
    });
    child.on('error', (error) => finish({ consumer: consumer.id, passed: false, error: error.message }));
    child.on('close', (code) => {
      try {
        const audit = JSON.parse(output),
          counts = audit.metadata?.vulnerabilities;
        if (!counts || audit.error || !Number.isInteger(counts.high) || !Number.isInteger(counts.critical))
          throw new Error(audit.error?.summary ?? 'Incomplete audit result');
        finish({
          consumer: consumer.id,
          passed: code === 0 && counts.high === 0 && counts.critical === 0,
          counts,
          findings: audit.vulnerabilities,
        });
      } catch (error) {
        finish({ consumer: consumer.id, passed: false, error: error.message, diagnostic: diagnostic.slice(-2000) });
      }
    });
  });
  results.push(result);
  console.log(
    `${result.passed ? 'PASS' : 'FAIL'} ${consumer.id} dependency audit: ${result.counts ? JSON.stringify(result.counts) : result.error}`,
  );
}
const report = {
  checkedAt: new Date().toISOString(),
  threshold: 'high',
  passed: results.every((result) => result.passed),
  results,
};
await mkdir(resolve(values.report, '..'), { recursive: true });
await writeFile(resolve(values.report), JSON.stringify(report, null, 2) + '\n');
if (!report.passed) process.exitCode = 1;
