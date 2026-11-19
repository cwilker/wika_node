import { spawnSync, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve, dirname, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const { values } = parseArgs({ options: {
  'report-directory': { type: 'string' },
  'network-namespace': { type: 'boolean', default: false },
} });
const report = values['report-directory']
  ? resolve(values['report-directory'])
  : resolve(root, '../compatibility/reports/verifier-transport');
mkdirSync(report, { recursive: true });
const result = {
  schemaVersion: 1, passed: false, startedAt: new Date().toISOString(),
  node: process.version, platform: process.platform, architecture: process.arch,
  networkNamespace: values['network-namespace'],
  scope: values['network-namespace']
    ? 'Real UDP DNS, TCP, TLS, HTTP and actual peer addresses in an isolated Linux network namespace with test-only DNS API and CA adapters.'
    : 'Real local UDP DNS, TCP, TLS and HTTP parsing with test-only DNS API, route, CA and public-peer adapters.',
  limits: values['network-namespace']
    ? 'No production recursive resolver, deployment firewall, IPv6 network path or independent operators are validated.'
    : 'Public peer values are modeled except the actual loopback rejection. No production recursive resolver, OS routing/firewall, IPv6 network path or independent operators are validated.',
  sources: {},
};
writeFileSync(resolve(report, 'result.json'), JSON.stringify(result, null, 2) + '\n');
try {
  for (const path of ['src/fetch.mjs', 'integration/transport.test.mjs', 'integration/transport.mjs', 'package-lock.json']) {
    result.sources[path] = createHash('sha256').update(readFileSync(resolve(root, path))).digest('hex');
  }
  result.openssl = execFileSync('openssl', ['version'], { encoding: 'utf8' }).trim();
  const args = [process.execPath, '--test', '--test-reporter=tap', 'integration/transport.test.mjs'];
  if (values['network-namespace']) {
    if (process.platform !== 'linux' || process.getuid?.() !== 0) throw new Error('--network-namespace requires Linux root with unshare and iproute2');
    // Fixed commands affect only the child namespace; host routes and firewall stay intact.
    args.unshift('unshare', '--net', '--', 'sh', '-ec',
      'ip link set lo up; ip address add 8.8.8.8/32 dev lo; ip address add 1.1.1.1/32 dev lo; export WIKA_TRANSPORT_NETWORK_NAMESPACE=1; exec "$@"',
      'wika-transport');
  }
  const child = spawnSync(args[0], args.slice(1), {
    cwd: root, encoding: 'utf8', timeout: 60_000, maxBuffer: 4 * 1024 * 1024,
    env: { ...process.env, WIKA_TRANSPORT_NETWORK_NAMESPACE: '0' },
  });
  const output = `${child.stdout ?? ''}${child.stderr ?? ''}`;
  writeFileSync(resolve(report, 'transport.tap'), output);
  process.stdout.write(output);
  result.exitCode = child.status; result.signal = child.signal;
  result.scenarios = [...output.matchAll(/^ok \d+ - (.+)$/gm)].map(match => match[1]);
  for (const field of ['tests', 'pass', 'fail', 'cancelled', 'skipped']) {
    const match = new RegExp(`^# ${field} (\\d+)$`, 'm').exec(output);
    result[field] = match ? Number(match[1]) : null;
  }
  if (child.error) throw child.error;
  if (child.status !== 0 || !result.tests || result.pass !== result.tests ||
      result.fail !== 0 || result.cancelled !== 0 || result.skipped !== 0 || result.scenarios.length !== result.tests) {
    throw new Error('Transport suite did not pass every scenario; see transport.tap');
  }
  result.passed = true;
} catch (error) {
  result.error = error.message; process.exitCode = 1;
} finally {
  result.finishedAt = new Date().toISOString();
  writeFileSync(resolve(report, 'result.json'), JSON.stringify(result, null, 2) + '\n');
  console.log(`Transport evidence: ${relative(process.cwd(), report)}`);
}
