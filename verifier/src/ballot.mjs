import { keccakAsU8a } from "@polkadot/util-crypto";
export function codecBytes(codec) {
  const value = codec.toJSON();
  return typeof value === "string"
    ? Buffer.from(value.replace(/^0x/, ""), "hex")
    : Buffer.from(value.map(Number));
}
export function voteMessage(vote, proof) {
  return Buffer.concat([
    Buffer.from(vote ? "1,," : "0,"),
    vote ? Buffer.from(proof) : Buffer.alloc(0),
  ]);
}
/** Matches the runtime SCALE tuple, including the entire on-chain request context for no votes. */
export function reportMessage(api, url, at, requester, context, vote, proof) {
  return Buffer.concat([
    Buffer.from("wika/safe-verification/v1"),
    Buffer.from(api.genesisHash.toU8a()),
    Buffer.from(api.registry.createType("Bytes", Array.from(url)).toU8a()),
    Buffer.from(at.toU8a()),
    Buffer.from(requester.toU8a()),
    Buffer.from(context.toU8a()),
    Buffer.from(
      api.registry
        .createType("Bytes", Array.from(voteMessage(vote, proof)))
        .toU8a(),
    ),
  ]);
}
export function prepareBallot(
  api,
  pair,
  url,
  at,
  requester,
  context,
  vote,
  proof,
) {
  const evidence = vote ? Buffer.from(proof) : Buffer.alloc(0);
  const salt = pair.sign(
    reportMessage(api, url, at, requester, context, vote, evidence),
  );
  const hash = keccakAsU8a(
    Buffer.concat([voteMessage(vote, evidence), Buffer.from(","), salt]),
  );
  return {
    vote,
    proof: Buffer.from(evidence).toString("hex"),
    salt: Buffer.from(salt).toString("hex"),
    hash: Buffer.from(hash).toString("hex"),
  };
}
