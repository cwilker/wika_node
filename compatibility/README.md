# Node consumer compatibility gate

This gate checks the current node against the React/core plugin, SubQuery ETL, and browser wallet v2. A node change fails with the consumer name and the exact requirement that no longer holds. It combines metadata contracts with real consumer behavior; a runtime version bump alone is not a compatibility decision.

## Quick check against a running node

From `wika_node/compatibility`, using Node 22.12+:

```sh
npm ci
npm test
WIKA_NODE_URL=ws://127.0.0.1:9944 npm run check:contracts
```

The default workspace layout is the four sibling repositories in this project's folder. Override it with `-- --workspace /path/to/folder`. Contract checks only read metadata/RPC capabilities and do not send transactions. They normally take seconds and do not require Postgres or installing the consumers.

A focused check can select an explicit contract:

```sh
npm run check:contracts -- \
  --endpoint ws://127.0.0.1:9944 \
  --contract ../../wika_etl/compatibility/node-contract.json
```

The node's supported contracts are also checked by default. The JSON report is written to `reports/contracts.json`. Missing endpoints, missing contracts, malformed requirements, connection failures, and incompatibilities return a nonzero exit code.

## Full behavior gate

Build the node binary once, and install dependencies in each consumer's package directory:

```sh
# From the workspace containing the four repositories:
npm ci --prefix wika_node/compatibility
npm ci --prefix wika_plugins_react
npm ci --prefix wika_etl/subql_wika
npm ci --prefix wika_browser_ext_react/v2

cd wika_node
cargo build --locked --release -p wika-node
cd compatibility
```

The full ETL test requires a **dedicated test Postgres** (version 16 in CI), with permission to create schemas and the `btree_gist` extension. For example:

```sh
docker run --rm --name wika-compat-postgres \
  -e POSTGRES_PASSWORD=postgres -p 127.0.0.1:15433:5432 \
  postgres:16-alpine
```

In another terminal, from `wika_node/compatibility`:

```sh
WIKA_TEST_DB_PORT=15433 npm run check:all

# Or select one consumer; plugin/browser do not require Postgres:
npm run check:consumer -- --consumer plugin
npm run check:consumer -- --consumer browser
WIKA_TEST_DB_PORT=15433 npm run check:consumer -- --consumer etl
```

Every consumer gets its own freshly started temporary development node, random RPC/P2P ports, funded development accounts, and isolated test URLs. The runner stops each node afterward and continues to report other consumers if one fails. ETL tests create and remove a dedicated schema. Expected full runtime is several minutes, primarily ownership verification windows and builds.

Options: `--workspace /path/to/folder`, `--node-binary /path/to/wika-node`, `--consumer-root /path/to/one/repo`, and `--report-directory /path/to/reports`. CLI paths resolve from the current working directory. The one-consumer root override is useful in CI.

Postgres environment: `WIKA_TEST_DB_HOST` (default `127.0.0.1`), `WIKA_TEST_DB_PORT` (required for ETL), `WIKA_TEST_DB_USER/PASS/NAME` (default `postgres`). Never point the behavior gate at a production database. The runner creates the required extension in this test database.

## What each consumer owns

Each consumer repository contains:

- `compatibility/node-contract.json`: the node surfaces actually consumed, with readable type shapes and behavioral expectations.
- `compatibility/suite.json`: the commands that exercise that consumer, including typechecking/builds and a live node test.
- `compatibility/run.mjs`: the `npm run test:compatibility` entry point.
- `.github/workflows/node-compatibility.yml`: consumer pull requests/pushes run the shared node compatibility workflow.

The React component is checked through its shared core, component tests, package builds, and real chain signing. The ETL is checked through the actual SubQuery indexer, Postgres, and GraphQL query service, including ownership emitted during block initialization. The wallet is checked through its real router, vault/keyring, chain subscriptions, private bucket reads, transaction history reconciliation, and dapp signer. Only browser platform APIs/approval clicks are simulated; node APIs and consumer business logic are real. Chrome and Firefox builds are also required.

The wallet's existing Playwright UI suite remains separate. This gate does not automate the installed browser extension or test Neo4j/Elasticsearch projections. It does not promise to detect unspecified economic/behavioral changes: add a behavior assertion whenever a new consumer assumption is introduced.

To use a running development node directly, run `npm run test:compatibility` from a consumer's package directory with `WIKA_NODE_URL` set. The helper defaults to the sibling `wika_node`; set `WIKA_NODE_CHECKOUT` for another checkout. Install its compatibility dependencies first. Required environment variables are enforced before live tests, so CI cannot become green because a live test silently skipped.

## Contract comparison rules

Requirements name calls/events/storage/constants/errors, supported extrinsic version and signed extensions, token properties, and required RPC methods. The checker resolves portable metadata type IDs into structural shapes instead of comparing an entire metadata dump.

- Missing APIs, argument order/number/name changes, integer widths, storage tuple shapes, absence behavior, required fields, and URL limits fail.
- The wallet additionally requires URL keys last and `Blake2_128Concat` hashing, because its private bucket lookups depend on that layout.
- Calls, events, tuple positions and signing extension order are checked conservatively. A change may need review even if a newer client could accommodate it.
- Unrelated API additions, extra named struct fields/enum variants, documentation edits, portable type-ID renumbering, and numeric pallet/call/event indexes are ignored. Consumers resolve named APIs through metadata.
- Mutable values such as like price are generally checked by type, then read and exercised by the actual consumer. Values hardcoded by a consumer, such as the URL byte limit, are checked explicitly.
- Stable API codec types such as `DispatchError` are recognized by name rather than freezing every SDK error variant. Live tests exercise error/signing decoding.

`npm test` includes deliberate breaking mutations that prove the gate fails for changed event fields, hashing, URL limits, RPC methods, signing extensions, and storage types. It also proves harmless additions and type-ID renumbering pass.

## CI and rollout

`wika_node/.github/workflows/ci.yml` builds/tests the candidate node once and uploads that exact executable. A non-fail-fast consumer matrix downloads it and runs this gate. Image publication depends on all consumers passing. Feature branch pushes and pull requests both run checks. No external notification service or dispatch token is required.

The reusable `consumer-compatibility.yml` workflow can also build a configured baseline node when called from a consumer repository. The consumer pull request's code is tested against that node. Every job uploads JSON reports, tested Git SHAs, node/service logs, and a GitHub job summary.

**These workflow files are local until committed and pushed.** Initial rollout requires:

1. Push the node's reusable workflow/harness to the configured node ref (`polkadot-sdk-upgrade` initially). The initial cross-repo checks will fail until consumer branches contain their new contract/suite files.
2. Push each consumer's changes to the refs listed in `consumers.json`. The initial defaults follow this workspace: plugin/ETL `master`, wallet `v2-rebuild`.
3. Verify the CI runs and configure branch protection to require all three consumer compatibility checks before merging node changes, plus the `Node compatibility` check before merging consumer changes.

Update repository names/refs in `consumers.json` and consumer workflow `uses`/`node_ref` values if publishing to forks or different branches. All configured repositories must be accessible to GitHub Actions. For a release, pin the consumer refs and the reusable workflow/node ref to reviewed tags or commit SHAs. This prevents a moving branch from silently changing the tested release. The current defaults deliberately track the in-progress project branches; they are not claims that these libraries are already released.

## Preserve supported consumers

`compatibility/supported/*.json` records the initial Wika 4.x consumer contracts. The gate checks them **in addition to** the current consumer's requirements, even if the latter were loosened during coordinated development. Node CI also checks the same consumer's supported contract from the pull request base commit (or previous pushed commit), so changing a snapshot in the candidate PR cannot hide a break. The first rollout has no previous snapshot; current contracts still run.

Do not regenerate supported contracts just to turn a failing job green. An intentional break requires a migration/release decision: update and release the affected consumers, document which old versions are no longer supported, and explicitly change the supported-version policy. A breaking node change should remain blocked while its previously supported contracts are still required. Add separately pinned consumer profiles when maintaining multiple released client lines.

A passing contract does not prove a storage migration is safe for existing chain state. Runtime upgrade/migration tests and deployment checks are still needed for that separate concern.

### Shared safe-claim fixtures

The plugin behavior suite also compares its vendored page identity and proof fixtures with `fixtures/page-identity-v1.json` and `fixtures/claim-proof-v1.json` in this node checkout. Missing or different fixtures fail the suite. Rust, core and verifier tests exercise those same fixtures; changing a fixture requires corresponding consumer implementation changes, not only a metadata contract update. The new safe-claim flow itself is still under construction.


### Safe-claim release coverage (runtime 402)

Plugin and extension contracts include every claim call and storage shape their shared core reads. The ETL declares `requirements.exhaustiveEvents: ["owners"]`: new event variants require review as well as removed/changed signatures. This catches new financial mutations that would otherwise leave old signatures intact. Unknown owners events also stop ETL replay.

Install dependencies in `../verifier` for the development website-fixture worker. The plugin suite exercises funded backlog, persistent proof after worker restart, refresh, replacement and historical withdrawals. ETL runs actual SubQuery → PostgreSQL → GraphQL replay from block 1. The extension runs real trusted-background signing and Chromium popup/injected-provider flows, builds both browsers and enforces its initial bundle budget. CI installs verifier dependencies and Chromium. Local macOS may require `CHROME_PATH` for a compatible Chromium executable.

`check:all` isolates each consumer in its own temporary node. Do not run consumer behavior suites concurrently against one chain: the same funded accounts/reviewer keys would conflict. PostgreSQL must be dedicated to tests; each ETL test creates/removes its schema. Linux CI uses Postgres 16; local validation may use newer compatible PostgreSQL.

These gates establish development integration compatibility. Production fees/bonds, hostile-network/load tests, benchmarked weights, independent verifier operations and external security review remain separate release requirements. Remote checks only apply once the related repository changes are committed and published to the configured refs.


### Dependency release gate

After functional compatibility, run `npm run check:dependencies` from this directory. It audits the three consumer lockfiles, records `reports/dependency-audit.json`, and fails on high/critical findings, malformed output, missing lockfiles or registry errors. Scope to one checkout with `--consumer etl --consumer-root /path/to/wika_etl`. The reusable CI workflow runs this as a separate required step, even if functional checks failed. Verifier CI also runs its dependency audit. A passing functional suite cannot override this gate.

The November 16 dependency remediation clears the high/critical gate for all three consumers: ETL 0 critical / 0 high (15 moderate / 15 low remain), core/React none, and extension moderate lint-tool findings only. ETL overrides require three fingerprint-checked SubQuery compatibility patches; see its `subql_wika/DEPENDENCIES.md`. The complete local ARM64 image gate passes. Remaining advisories, AMD64 image validation, full containerized financial indexing and other release requirements still need review before production. The workspace `docs/release-validation.html` keeps this separate from the completed implementation and functional test results.
