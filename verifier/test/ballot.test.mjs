import { test } from "node:test";
import assert from "node:assert/strict";
import { TypeRegistry } from "@polkadot/types";
import {
  cryptoWaitReady,
  sr25519PairFromSeed,
  sr25519Verify,
  sr25519Sign,
  keccakAsU8a,
} from "@polkadot/util-crypto";
import { reportMessage, prepareBallot, voteMessage } from "../src/ballot.mjs";
test("SCALE report encodes raw URL and binary evidence with length prefixes exactly once", async () => {
  await cryptoWaitReady();
  const registry = new TypeRegistry();
  const pair = sr25519PairFromSeed(new Uint8Array(32).fill(7));
  const genesis = registry.createType("H256", "0x" + "01".repeat(32)),
    api = { registry, genesisHash: genesis };
  const url = Buffer.from("https://x.test/a"),
    at = registry.createType("u32", 7),
    account = registry.createType("AccountId", "0x" + "02".repeat(32));
  const context = registry.createType("(u8,Bytes,u64,u8)", [0, [97], 10, 2]);
  for (const vote of [false, true]) {
    const proof = vote ? Buffer.alloc(32, 3) : Buffer.alloc(0);
    const message = voteMessage(vote, proof);
    const expected = Buffer.concat([
      Buffer.from("wika/safe-verification/v1"),
      Buffer.alloc(32, 1),
      Buffer.from([url.length << 2]),
      url,
      Buffer.from([7, 0, 0, 0]),
      Buffer.alloc(32, 2),
      Buffer.from(context.toU8a()),
      Buffer.from([message.length << 2]),
      message,
    ]);
    const actual = reportMessage(api, url, at, account, context, vote, proof);
    assert.deepEqual(actual, expected);
    const ballot = prepareBallot(
      api,
      { sign: (message) => sr25519Sign(message, pair) },
      url,
      at,
      account,
      context,
      vote,
      proof,
    );
    const salt = Buffer.from(ballot.salt, "hex");
    assert.equal(sr25519Verify(actual, salt, pair.publicKey), true);
    assert.equal(
      ballot.hash,
      Buffer.from(
        keccakAsU8a(Buffer.concat([message, Buffer.from(","), salt])),
      ).toString("hex"),
    );
  }
});
