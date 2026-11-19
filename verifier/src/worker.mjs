import {
  mkdir,
  readFile,
  writeFile,
  rename,
  open,
  unlink,
} from "node:fs/promises";
import { randomUUID } from "node:crypto";
import { dirname } from "node:path";
import { blake2AsU8a, blake2AsHex, decodeAddress } from "@polkadot/util-crypto";
import { codecBytes, prepareBallot } from "./ballot.mjs";
import { fetchPage } from "./fetch.mjs";
import { extractHeadProof } from "./proof-html.mjs";
import { verifyPageProof } from "./proof.mjs";
const same = (codec, pair) =>
  Buffer.from(decodeAddress(codec.toString())).equals(
    Buffer.from(pair.publicKey),
  );
/** Bounded active-request polling. No scan of claimed pages and no periodic page rechecks. */
export async function startVerifier({
  api,
  pair,
  stateFile,
  fixtures,
  log = () => {},
}) {
  if (!api.query.owners?.claimChecks)
    throw new Error("Node does not support signed claims");
  if (fixtures && !(await api.rpc.system.chainType()).isDevelopment)
    throw new Error("Fixture proofs are development-only");
  const verifier = await api.query.owners.verifiers(pair.address);
  if (verifier.isNone || !verifier.unwrap()[1].isTrue)
    throw new Error("Verifier key is not enabled on this chain");
  const identity = {
    genesis: api.genesisHash.toHex(),
    publicKey: Buffer.from(pair.publicKey).toString("hex"),
  };
  let state = { version: 1, ...identity, items: {} };
  try {
    state = JSON.parse(await readFile(stateFile, "utf8"));
    if (
      state.version !== 1 ||
      state.genesis !== identity.genesis ||
      state.publicKey !== identity.publicKey ||
      !state.items ||
      typeof state.items !== "object"
    )
      throw new Error(
        "Verifier state belongs to a different chain/key or has an unsupported schema",
      );
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  await mkdir(dirname(stateFile), { recursive: true, mode: 0o700 });
  const lockPath = stateFile + ".lock";
  const lock = await open(lockPath, "wx", 0o600).catch(() => {
    throw new Error(
      "Verifier state is locked. Confirm the previous process has stopped before removing its lock.",
    );
  });
  await lock.writeFile(String(process.pid));
  const releaseLock = async () => {
    await lock.close();
    await unlink(lockPath);
  };
  const save = async () => {
    const temp = stateFile + ".tmp-" + randomUUID();
    try {
      await writeFile(temp, JSON.stringify(state), {
        mode: 0o600,
        flag: "wx",
        flush: true,
      });
      await rename(temp, stateFile);
    } catch (error) {
      await unlink(temp).catch(() => {});
      throw error;
    }
  };
  let stopped = false,
    processing = false,
    pendingHead = null,
    stopHeads;
  const controller = new AbortController();
  let serial = Promise.resolve();
  // Serialize nonce allocation/submission, not block inclusion. All commits can land together.
  const send = (call) => {
    const next = serial.then(
      () =>
        new Promise((resolve, reject) => {
          if (stopped) {
            resolve();
            return;
          }
          let unsubscribe,
            settled = false,
            stopRequested = false;
          const stop = () => {
            // Terminal status/timeout can precede signAndSend's unsubscribe promise.
            // Remember cleanup so a late subscription cannot revive its watcher.
            stopRequested = true;
            clearTimeout(observationTimer);
            if (unsubscribe) {
              const release = unsubscribe;
              unsubscribe = null;
              release();
            }
            watchers.delete(stop);
          };
          const finish = (error) => {
            if (settled) return;
            settled = true;
            clearTimeout(timer);
            controller.signal.removeEventListener("abort", abort);
            error ? reject(error) : resolve();
          };
          const abort = () => {
            stop();
            finish(new Error("Verifier stopping"));
          };
          const timer = setTimeout(() => {
            stop();
            finish(new Error("Verifier transaction submission deadline"));
          }, 20_000);
          // Ready/broadcast acknowledges submission, not inclusion. Bound observation
          // even if the RPC never sends a terminal status; later heads read chain state.
          const observationTimer = setTimeout(() => {
            stop();
            log("Verifier transaction observation deadline; checking chain state on later heads");
          }, 120_000);
          controller.signal.addEventListener("abort", abort, { once: true });
          Promise.resolve()
            .then(() => call.signAndSend(pair, { nonce: -1 }, ({ status, dispatchError }) => {
              if (dispatchError) log("Verifier transaction rejected");
              if (status.isInvalid || status.isDropped || status.isUsurped)
                finish(new Error("Verifier transaction not accepted"));
              else if (
                status.isReady ||
                status.isBroadcast ||
                status.isInBlock ||
                status.isFinalized
              )
                finish();
              if (
                dispatchError ||
                status.isInBlock ||
                status.isFinalized ||
                status.isInvalid ||
                status.isDropped ||
                status.isUsurped
              )
                queueMicrotask(stop);
            }))
            .then(
              (unsub) => {
                unsubscribe = unsub;
                if (stopped || stopRequested) stop();
                else watchers.add(stop);
              },
              (error) => {
                finish(error);
                stop();
              },
            );
        }),
    );
    serial = next.catch(() => {});
    return next;
  };
  const watchers = new Set();
  async function tick(header) {
    const height = header.number.toNumber();
    const view = await api.at(header.hash);
    const chainTime = BigInt((await view.query.timestamp.now()).toString());
    const entries = await view.query.owners.requests.entries();
    if (entries.length > 256) throw new Error("Active request bound exceeded");
    const jobs = [];
    for (const [key, value] of entries) {
      if (value.isNone) continue;
      const url = codecBytes(key.args[0]);
      const page = url.toString("utf8");
      const [at, requester] = value.unwrap();
      const [contextOpt, roundOpt, commitOpt, revealOpt] = await Promise.all([
        view.query.owners.claimChecks(page),
        view.query.owners.rounds(page),
        view.query.owners.commits(page, pair.address),
        view.query.owners.reveals(page, pair.address),
      ]);
      if (contextOpt.isNone || roundOpt.isNone) continue;
      const context = contextOpt.unwrap(),
        round = roundOpt.unwrap();
      if (!round.members.some((member) => same(member, pair))) continue;
      const commitEnd = round.commitEnd.toNumber(),
        revealEnd = round.revealEnd.toNumber();
      const id = `${url.toString("hex")}:${at}:${Buffer.from(context.toU8a()).toString("hex")}`;
      const item = state.items[id];
      if (height > revealEnd) continue;
      if (revealOpt.isSome) {
        if (item) {
          delete state.items[id];
          await save();
        }
        continue;
      }
      if (height > commitEnd) {
        if (
          !item ||
          item.unavailable ||
          commitOpt.isNone ||
          item.lastReveal === height
        )
          continue;
        if (
          Buffer.from(commitOpt.unwrap().toU8a()).toString("hex") !== item.hash
        )
          throw new Error("Stored ballot does not match on-chain commitment");
        item.lastReveal = height;
        await save();
        await send(
          api.tx.owners.revealVerification(
            page,
            item.vote,
            [],
            Array.from(Buffer.from(item.proof, "hex")),
            Array.from(Buffer.from(item.salt, "hex")),
          ),
        );
        continue;
      }
      if (
        height <= at.toNumber() ||
        chainTime >= BigInt(context.expiresAt.toString())
      )
        continue;
      if (commitOpt.isSome || item?.unavailable || item?.lastCommit === height)
        continue;
      if (item) {
        item.lastCommit = height;
        await save();
        await send(
          api.tx.owners.commitVerification(
            page,
            Array.from(Buffer.from(item.hash, "hex")),
          ),
        );
      } else if (jobs.length < 4)
        jobs.push({
          id,
          url,
          page,
          at,
          requester,
          context,
          commitEnd,
          revealEnd,
        });
    }
    // Expired records contain only public evidence/signature salt; delete them boundedly.
    for (const [id, item] of Object.entries(state.items))
      if (height > item.revealEnd) delete state.items[id];
    if (Object.keys(state.items).length > 256)
      throw new Error("Verifier state bound exceeded");
    await Promise.all(
      jobs.map(async (job) => {
        let html;
        try {
          if (fixtures) {
            const map = JSON.parse(await readFile(fixtures, "utf8"));
            if (typeof map[job.page] !== "string")
              throw new Error("No development fixture");
            html = map[job.page];
          } else
            html = await fetchPage(job.page, { signal: controller.signal });
        } catch {
          state.items[job.id] = { unavailable: true, revealEnd: job.revealEnd };
          return;
        }
        const extracted = extractHeadProof(html);
        const expectedProof = codecBytes(job.context.proof);
        const digest = blake2AsU8a(expectedProof, 256);
        const valid =
          extracted.outcome === "extracted" &&
          (await verifyPageProof(extracted.proof, {
            genesisHash: identity.genesis,
            page: job.page,
            account: job.requester.toString(),
            acceptedDigest: blake2AsHex(expectedProof, 256),
          }));
        const item = {
          ...prepareBallot(
            api,
            pair,
            job.url,
            job.at,
            job.requester,
            job.context,
            Boolean(valid),
            digest,
          ),
          revealEnd: job.revealEnd,
          lastCommit: height,
        };
        state.items[job.id] = item;
      }),
    );
    // Persist all salts atomically before broadcasting any commitment.
    await save();
    for (const job of jobs) {
      const item = state.items[job.id];
      if (!item || item.unavailable || stopped) continue;
      const head = await api.rpc.chain.getHeader();
      if (head.number.toNumber() >= job.commitEnd) continue;
      await send(
        api.tx.owners.commitVerification(
          job.page,
          Array.from(Buffer.from(item.hash, "hex")),
        ),
      );
    }
  }
  let running = Promise.resolve();
  function enqueue(header) {
    pendingHead = header;
    if (processing || stopped) return;
    processing = true;
    running = (async () => {
      while (pendingHead && !stopped) {
        const head = pendingHead;
        pendingHead = null;
        try {
          await tick(head);
        } catch (error) {
          log(`Verifier round error: ${error.message}`);
        }
      }
    })().finally(() => {
      processing = false;
    });
  }
  try {
    stopHeads = await api.rpc.chain.subscribeNewHeads(enqueue);
    enqueue(await api.rpc.chain.getHeader());
  } catch (error) {
    stopHeads?.();
    await releaseLock();
    throw error;
  }
  return {
    async stop() {
      if (stopped) return;
      stopped = true;
      controller.abort();
      stopHeads();
      try {
        await running;
        await serial;
        for (const stop of watchers) stop();
        await save();
      } finally {
        await releaseLock();
      }
    },
  };
}
