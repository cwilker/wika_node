# Safe page claim verifier

The verifier now drives the runtime's signed-proof requests. The node owns rewards, fees, bonds, thresholds and state transitions; this process only observes publishing proofs and commits/reveals signed reports. It never scans owned pages or fetches on each like.

With Node 22.12+ and a fresh local runtime-402 development chain:

```sh
npm ci --ignore-scripts
npm test
npm start -- --dev --endpoint ws://127.0.0.1:9944
```

The development key is `//Alice//ownr`, separately funded/registered in development genesis. A real operator supplies `--key-file /private/path/to/seed-uri` and a dedicated funded registered sr25519 key. One key contributes one vote. Four local keys can exercise a 3-of-4 quorum; production requires independent operators and reviewed configuration.

Use `--state /private/path/to/verifier.json` for commit/reveal state. Salts are atomically written with private permissions and flushed before commit broadcast. An exclusive lock prevents two processes from writing the same state file. After an ungraceful crash, confirm the old process stopped before removing only its stale `.lock`; retain the JSON for reveal recovery. Chain/key mismatches fail closed. The daemon caps concurrent observations at four and serializes transaction submission to avoid nonce races. Those development capacities still need load testing.

## Transaction cleanup and bounded work

The worker observes at most four new pages per tick and rejects an active-request set above 256. A controlled-chain stress test exercises all 256 requests in 64 batches, using real ballot signing and durable state files. This checks batching and resource cleanup, not production throughput or runtime weights.

Transaction status subscriptions are released exactly once, including when a terminal callback arrives before the API returns its unsubscribe function. Late unsubscribe promises after a submission timeout are also released. Ready/broadcast status allows the submission queue to advance; it is not proof of block inclusion. A status observer that never receives a terminal update is released after two minutes. This only ends that RPC observation: it does not cancel the transaction or establish its result. The worker continues to read chain state on later heads.

Regression tests cover the full queue, invalid/dropped/usurped status, late subscription cleanup, the observation deadline, shutdown and synchronous signing failure. Deadline tests use a controlled clock. An earlier version leaked subscriptions in three reproduced cases; the fix adds no chain calls or changes to rewards, proofs or voting thresholds.

## Proof and transport

- `fetchPage`: one validated DNS resolution, pinned public connection destination, normal HTTPS hostname/certificate verification and actual-peer checks. Port 443 only; no redirects, proxy, connection pooling or compression. Preliminary limits: 15 seconds including DNS, 2 MiB body, 16 KiB headers.
- `extractHeadProof`: exactly one designated metadata field in an explicit server HTML head. No DOM execution, body/footer/comment/script/template markers or implied-head placement. Unrelated body parse errors do not affect the proof.
- `verifyPageProof`: signature and chain/page/wallet/digest binding. Initial activation also checks its chain-time deadline in the runtime; later requests observe the accepted persistent tag with a new attestation context. Its original initial expiry does not expire an accepted tag.
- `ballot`: binds the complete on-chain request context, including negative reports, to prevent cross-request or fork/envelope replay. API vector arguments are explicit byte arrays, not raw SCALE-decoded buffers.

The wallet signing preimage is `wika/page-claim/v1\0` followed by SCALE:

```
version:u8=1, method:u8=1, genesis:[u8;32], page:BoundedVec<u8,254>,
account:AccountId32, nonce:[u8;32], expires_at:u64 (Unix milliseconds)
```

Content is `wika-claim-v1:<lowercase body hex>.<lowercase SCALE MultiSignature hex>`, capped at 2048 bytes. Standard `<Bytes>` wrapping is accepted. The digest is Blake2-256 of the entire UTF-8 content attribute. Safe verifier report serialization is defined by `Pallet::round_message`; the worker includes the complete SCALE `ClaimCheck` in both yes and no signatures. Rust/core/verifier share fixtures.

## Live development check

Build `@wika/core` in the sibling plugin repo and start an isolated development node, then:

```sh
WIKA_NODE_URL=ws://127.0.0.1:9944 node integration/safe-claims.mjs
```

This submits development transactions, uses a temporary local HTML fixture map, verifies funded backlog/claim/refresh/cooperative replacement/historical withdrawal, and restarts the worker with its state file. It uses short commit/reveal windows; the 14-day/90-day policies are exercised with runtime clock tests. It creates unique pages and removes only its own temporary files. `--fixtures`/development keys are refused on non-development chains. Fixtures never enable unsafe public HTTP fetching.

For four separate verifier processes with separate API connections, keys, fixture maps and durable state, build `target/release/wika-node` and run from the node repository:

```sh
npm ci --prefix verifier --ignore-scripts
npm run --prefix verifier test:adversarial -- --report-directory ../compatibility/reports/verifier-adversarial
```

This runner always starts its own temporary runtime-402 development chain; it accepts no external chain endpoint. It tests 3 yes/1 no, 1 yes/3 no, 2 yes/2 no, 2 yes/2 unavailable, 3 yes/1 unavailable, 1 yes/3 unavailable, process death after commitment with durable reveal recovery, and key revocation during a snapshotted round. It checks finalized outcomes, exact electorate/threshold/matching counts, distinct reveals, retry eligibility, retained unclaimed rewards and total reward liability. Missing fixtures represent unavailable observations and must abstain. Different unrelated HTML per observer must not split affirmative votes. Temporary development keys/state/fixtures are removed and only reports/logs remain. Allow roughly 15 minutes; chain finality and the deliberately bounded windows use real blocks. CI runs the same check against its newly built binary and retains reports even on failure.

These are four local processes using controlled HTML fixtures, not independent production operators or hostile DNS/TLS transport tests.

## DNS and TLS transport integration

With Node 22.12+ and OpenSSL on the path, run from the node repository:

```sh
npm run --prefix verifier test:transport
```

The 19 scenarios use a disposable UDP DNS authority and real TCP/TLS/HTTP traffic. They cover a DNS answer changing from public to private between requests, mixed private/public answers, untrusted/wrong-host/expired certificates, SNI and hostname preservation, peer mismatch, redirects to metadata endpoints, compression, header/body bounds, invalid UTF-8, truncated bodies, stalled TLS cancellation, unanswered DNS cancellation and the actual 15-second total deadline. Every failure must remain `FetchUnavailable`. Temporary certificate keys are generated per run and removed afterward. No real public address is contacted.

On macOS and ordinary unprivileged hosts, test-only adapters route the public address label to a loopback origin and model the public peer for positive/HTTP cases. The actual loopback peer rejection uses its real address. TLS certificate/hostname validation stays enabled with a disposable test CA. This mode passed locally; it does not validate OS routing or deployment egress controls.

For actual public peer addresses, the runner also supports an isolated Linux network namespace:

```sh
sudo "$(command -v node)" verifier/integration/transport.mjs --network-namespace \
  --report-directory compatibility/reports/verifier-transport
```

This requires Linux root, `unshare`, `iproute2` and OpenSSL. It creates a child network namespace with only loopback and two public IPv4 aliases. Origins listen on port 443 there; socket peer addresses are not overridden. The namespace has no external network interface and disappears when its processes exit. Host routes, resolvers and firewall rules are unchanged. The DNS API and test CA adapters remain. CI is configured for this mode; a Linux/remote CI run is still pending. Neither mode validates a production recursive resolver, IPv6 routing or deployment firewall rules.

The runner saves TAP, source fingerprints, counts, host details and explicit scope in `compatibility/reports/verifier-transport/result.json`. A timeout, missing counts, failed/skipped/cancelled test or namespace setup error fails the command. Use `--report-directory` to choose a different path (relative to the command's current directory; npm runs scripts from `verifier`). No production fetch bypass was added.

## Release boundary

The node lifecycle, financial ETL replay, core/React controls, local owners watchlist dashboard and native extension claim/reward workflows are implemented. Live consumer suites use the external verifier and signed-head proof ceremony. The plugin suite includes this lifecycle test; the ETL checks actual PostgreSQL/GraphQL financial replay; the extension checks trusted background signing and Chromium popup flows. The node compatibility contracts include safe-claim calls/storage, and the ETL reviews the complete owners event set. See `../compatibility/README.md` and the workspace roadmap for current validation results. These are development integration checks, not production approval.

Development placeholders: 5 WIKA verification fee, 1 WIKA replacement bond, two no-quorum retries, 24-hour maximum initial/request lifetime, seven-day completion allowance after the 14-day window, four active requests per wallet and 256 globally. These are not approved production economics. Runtime weights/capacities need benchmarking and review. The original mocked fetch tests and 19 local DNS/TLS transport scenarios pass. The Linux namespace mode, deployment egress isolation, production resolver/IPv6 behavior, real independent operators and external review remain release requirements.
