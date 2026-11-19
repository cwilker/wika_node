# Wika Node

The blockchain behind the [Wika Network](https://www.wika.network): like web pages with WIKA, prove you own a page, and collect the likes it receives.

Version 4 moves the node from Substrate `monthly-2021-08` to the Polkadot SDK (`polkadot-stable2609`). The node is built from the SDK's solochain template, and the three Wika pallets are rewritten with the current FRAME macros. Runtime 402 adds signed page claims and financial escrow; legacy marker claims are disabled. Consumers must support the current metadata contracts. See [What changed in 4.0](#what-changed-in-40).

## Quick start

You need Rust (via [rustup](https://rustup.rs)), `protoc`, and clang. On macOS: `brew install protobuf`. On Debian or Ubuntu: `apt install clang libclang-dev protobuf-compiler`.

```sh
rustup target add wasm32v1-none
rustup component add rust-src
cargo build --release
./target/release/wika-node --dev
```

On macOS, if the RocksDB build cannot find `libclang.dylib`, run `export LIBCLANG_PATH=/Library/Developer/CommandLineTools/usr/lib` and try again.

The node serves RPC over HTTP and WebSocket on `ws://127.0.0.1:9944`. Point the Wika browser extension (Settings → Network → custom endpoint) or [polkadot.js apps](https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9944) at it.

A dev chain has the following set up:

- **Block production:** Alice authors blocks. Sudo is Alice.
- **Funded accounts:** the well-known dev accounts (`//Alice`, `//Bob`, …, plus their `//stash` accounts) are funded with about 1.15 M WIKA each.
- **Ownership verification:** `//Alice//ownr` is a registered verifier, and `--dev` puts its key in the node's keystore. Runtime 402 uses the separate [signed-proof verifier](verifier/README.md) for safe page claims; start that process alongside the node. The verifier is a separate account from Alice, so its commit and reveal transactions never compete with Alice's for nonces.
- **Logs:** add `-lLIKE=debug -lOWNERS=debug` to watch the pallets work.

To run with Docker:

```sh
docker compose up --build        # or: docker build -t wika-node . && docker run -p 9944:9944 wika-node --dev --rpc-external --rpc-cors=all
```

## Tests

```sh
cargo test -p pallet-likes -p pallet-owners -p pallet-authorities
```

The pallet tests cover:

- **Likes:** payment splits and queue repayment.
- **Owners:** the commit and reveal timing windows, salt and proof checks, the tally, and clean-up. Two tests run the offchain worker against a mocked page: one with the ownership mark, one without.
- **Authorities:** changes are pushed to Aura and GRANDPA.

For an end-to-end test of a wallet against a real node, the browser extension's Playwright suite runs Like, a duplicate Like, Claim and Send when `WIKA_NODE_URL=ws://127.0.0.1:9944` is set.

## Consumer compatibility

Node CI also checks the core/React plugin, ETL, and browser extension against the candidate binary before publishing an image. Each consumer declares its required calls, event fields, storage types, RPC methods, and limits; live tests exercise its actual node integration. Failures identify the affected consumer and requirement.

See [the compatibility gate](compatibility/README.md) for local commands, reports, supported baselines, CI setup, and rollout. For a quick read-only check against a running node:

```sh
cd compatibility
npm ci
WIKA_NODE_URL=ws://127.0.0.1:9944 npm run check:contracts
```

## Layout

```
node/                 CLI, chain specs (dev, local, test, main), service, RPC
runtime/              the runtime: Aura + GRANDPA + balances + sudo + the Wika pallets
  src/genesis_config_presets.rs
pallets/likes         like a URL, split the payment, repay earlier likers
pallets/owners        signed claims, ownership periods, escrow and verification rounds
pallets/authorities   sudo-managed set of block authors and finality voters
libs/traits           OwnershipRegistry, shared by likes and owners
validators/           JSON-RPC payloads for inserting a validator's keys
```

## How it works

### Likes (`pallets/likes`)

`likes.like(url, url_ref, num_likes)` charges `LikePrice` (1 WIKA by default) per like and splits the payment:

| Share | Default | Goes to |
| --- | --- | --- |
| `ShareAuthor` | 33% | funded escrow for the current ownership period, or the unclaimed page balance |
| `ShareReferrer` | 33% | the owner of `url_ref` (the page that sent the user), or the owners pot if it is unclaimed or the liker owns it |
| `SharePreviousLikers` | 33% | earlier likers of `url`, front of the queue first |
| remainder | 1% | the likes pot |

Each liker joins the URL's queue and is owed `num_likes × NumRoundsToRewardLikers` (4) likes' worth of the previous-likers share. Later likes pay it back. An account can like a URL once (`AlreadyInQueue`), with at most `MaxLikes` (100) likes per call. URLs must be shorter than 255 bytes.

Storage read by wallets:

- `likes.urls(url)` is `(total likes, front of queue, back of queue)`.
- `likes.likes(account, url)` is `(likes before mine, my likes, likes still owed to me, next in queue)`.
- `likes.likePrice()`.

### Ownership (`pallets/owners`)

Current runtime-402 claims use a wallet-signed proof in the server-rendered HTML head, observed by the external verifier. See [safe page claims](#safe-page-claims-runtime-402) and the [verifier guide](verifier/README.md). The following describes the older marker protocol retained in the source; it is disabled in the current runtime.

#### Legacy marker protocol (disabled)

1. **Request.** A user publishes the mark `wika.network/author/<their 64-hex public key>` somewhere on the page, then calls `owners.requestUrlCheck(url)`, which costs `RequestPrice` (5 WIKA).
2. **Commit.** Every node that holds an enabled verifier's `ownr` key fetches the page in its offchain worker. It follows up to 3 redirects and reads up to 2 MB. It then votes and commits `keccak256(vote,intro[,proof],salt)` within `NumBlocksToCommit` (5) blocks. The salt is the verifier's signature over its vote.
3. **Reveal.** During the next `NumBlocksToReveal` (5) blocks the verifier reveals its vote. The runtime checks four things:
   - the salt is the sender's signature;
   - the reveal matches the commit;
   - for a yes vote, the proof starts with the prefix;
   - the proof contains the requester's key.
4. **Tally.** One block later the votes are tallied into `owners.results(url)`. The URL gets an owner in `owners.owners(url)` when three conditions hold:
   - most votes are yes;
   - the majority holds more than `PrctNeededForAgreement` (66%) of the votes;
   - the majority has at least `MajorityMin` (1) members.

   On a dev chain the whole round takes about 12 blocks, which is 72 seconds.
5. **Clean-up.** `NumBlocksToDelete` (100) blocks later the request, commits, reveals and result are deleted. The ownership stays.

To make a node a verifier:

1. Insert an `ownr` key into its keystore. See `dev_keys/alice_ownr.json` and `validators/`.
2. Call `owners.addVerifier(account)` through sudo.
3. Fund the account, because it pays fees for its commits and reveals.

### Authorities (`pallets/authorities`)

Sudo calls `authorities.addAuthority(account, sr25519, ed25519)`, `disableAuthority` and `enableAuthority`.

- **Aura:** the change applies from the next block.
- **GRANDPA:** the change is scheduled. GRANDPA allows one pending change at a time, so a second change too soon fails with `ChangePending`.
- **Safety check:** the pallet refuses to disable the last authority.

## Chain specs

| `--chain` | What it is |
| --- | --- |
| `dev` (or `--dev`) | Alice only, instant setup, verifier included |
| `local` | Alice and Bob as authorities; start each with `--alice` / `--bob` |
| `test` | a new chain with the original 2021 testnet authorities and sudo key |
| `main` | a new chain with the original 2021 mainnet authorities and sudo key |
| a path | any spec exported with `wika-node export-chain-spec --chain <id>` |

Every spec declares `tokenSymbol: WIKA`, `tokenDecimals: 12` and `ss58Format: 42`.

Live networks still need two things:

- **Bootnodes.** The 2021 bootnode is not included.
- **Verifiers in genesis or via sudo.** Validators start with `--validator` and insert their `aura`, `gran` and `ownr` keys (`validators/add_keys`, RPC on port 9944).

## What changed in 4.0

- **Polkadot SDK `polkadot-stable2609`**, with crates pinned in the workspace `Cargo.toml` and resolved in `Cargo.lock`. The node and runtime come from the SDK's solochain template.
- **Rust and runtime:** the code builds on stable Rust; nightly is no longer needed. The runtime is built for `wasm32v1-none`.
- **Pallet macros:** the pallets use `#[frame_support::pallet]` instead of `decl_module!` and `decl_storage!`. Their calls and storage keep their names. Events now have named fields (`Liked { who, url, num_likes }`).
- **Storage queries:** storage that had no meaningful default is now `OptionQuery`, so missing entries read as `None` rather than a tuple of zeros. URLs in storage are bounded at 255 bytes.
- **New errors, appended after the existing ones so their indices don't change:**
  - `likes.NoLikes`, for zero likes;
  - `owners.TooManyRequests`, at most 64 requests per block;
  - `authorities.TooManyAuthorities`, `NoAuthoritiesLeft` and `ChangePending`.
- **Removed calls:** the unused `owners.test_tx` call is gone (other call indices are unchanged), along with the unused `NumChecksRequired` constant.
- **Fixes in the owners pallet:**
  - The offchain worker no longer panics on pages shorter than 128 bytes or on a mark near the end of the page.
  - The on-chain reveal now checks that a yes vote's proof contains the requester's key; this was a `TODO` in 2.x.
  - Verifier statistics computed during the tally are now saved; 2.x dropped them.
  - Redirects are followed.
  - Commits and reveals are signed with the verifier key that was checked, rather than any key in the keystore.
- **Fixes in the likes pallet:**
  - A failed transfer now aborts the like and rolls it back, where 2.x panicked inside the runtime.
  - Transfers keep the sender above the existential deposit.
- **Fixes in the authorities pallet:**
  - It now really changes the Aura and GRANDPA authority sets. In 2.x it only overrode the runtime API, so GRANDPA never saw changes.
  - Disabled authorities now leave the set.
- **Development and packaging:**
  - Dev nodes get a verifier key automatically.
  - Chain specs carry token properties.
  - The Docker image runs as a non-root user.
  - The JSON-RPC server is a single port, 9944, for HTTP and WebSocket.

This is a new chain. It does not migrate state from the 2021 testnet, whose runtime cannot be upgraded in place from such an old Substrate version.

## Not done yet

- **Benchmarked weights.** Weights are conservative hand-set estimates, and the benchmarking CLI is not built into the node.
- **Fee-free verifier transactions.** Verifier commits and reveals pay normal fees, so verifier accounts need funds.
- **Bootnodes and telemetry** for a public network.

## Safe page claims (runtime 402)

The node now holds funded author allocations in separate escrow, assigns the first backlog once, and tracks ownership periods. Current withdrawals reuse verification for 90 days; closed periods remain payable to their original wallets without current website control. Valid replacement claims open a 14-day frozen window and require a fresh final check; cooperative approval by both wallets can waive the wait. Earlier earnings never move to the replacement wallet.

Signed-proof requests, bonds, two fee-exempt no-quorum retries, explicit outcomes and bounded active work are implemented. Activation happens at block finalization so timestamp-based policy uses the current block. Legacy marker claims are disabled; start from fresh development genesis, not an existing chain upgrade. No migration or deletion of existing chain data is performed by this work.

See [verifier setup and integration validation](verifier/README.md). Financial ETL projections, a basic local owners dashboard, native extension workflows and the full consumer behavior suites are implemented and passed the local combined functional gate. The high/critical dependency gate now passes. Production release still requires review of remaining moderate/low ETL advisories, AMD64 image verification/full containerized financial indexing (local ARM64 image gate passes), hostile-network integration, benchmarks, fee/bond configuration review and independent operator/review arrangements. The workspace release validation report records the evidence and limits.


### Signed host referrals (runtime 403)

Verified current page owners authorize up to four sr25519 host keys through `likes.authorizeHost`, and revoke a generation through `likes.revokeHost`. `likes.likeWithReferral` binds a paying account, exact source and target, chain, five-minute chain-time expiry, request ID and runtime economic commitment. `ReferralApi_quote` is a read-only finalized-state runtime API. All money uses integer units. Allocation parameters are unchanged by this feature.

`likes.like` keeps its call shape for direct likes. Nonempty legacy referral arguments are rejected at and after the runtime's configured `ReferralActivation` height (zero in this development build). Set the intended activation height when preparing a staged chain upgrade, release updated consumers first, and communicate the cutoff. No host keys are installed automatically. Storage version 1 initializes the new namespace without changing balances or accrued rewards.

Replay state uses 302 bounded second buckets with at most 64 consumed IDs each. Admission prunes its bucket before reserving a slot. Idle cleanup visits one bucket with a bounded weight budget. Requests cannot be replayed after pruning because chain-time expiry remains mandatory. Transactional execution rolls back transfers, replay reservations and like state on failure. Admission capacity and conservative weights still require calibration and benchmarking on release reference hardware before production activation.

The signed format and shared fixture are in `pallets/likes/src/referrals.rs` and `compatibility/fixtures/host-referral-v1.json`. The native integration suite belongs to the SDK checkout (`test:referrals-live`). Host signatures authenticate authorization; they cannot prove a visitor saw a page or prevent a modified wallet from substituting a different valid request for that payer.

## Page payout policies

Owner-configurable policies and economic terms v2 are implemented in runtime 404. Activation is disabled until root supplies explicit release rules and a future boundary through `likes.scheduleEconomics`. Existing pages resolve lazily to a frozen legacy profile; new policies lock on first successful payment. See the [implementation and release guide](../docs/page-economics-implementation.html).
