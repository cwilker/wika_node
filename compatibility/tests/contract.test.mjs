import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { TypeRegistry } from "@polkadot/types";
import { checkContract, validateContract } from "../src/contract.mjs";
import { describeType, readSurface } from "../src/surface.mjs";
import { command } from "../src/process.mjs";

const plugin = JSON.parse(
  await readFile(new URL("../supported/plugin.json", import.meta.url)),
);
const etl = JSON.parse(
  await readFile(new URL("../supported/etl.json", import.meta.url)),
);
const browser = JSON.parse(
  await readFile(new URL("../supported/browser.json", import.meta.url)),
);
function surface() {
  const result = {
    calls: {},
    storage: {},
    events: {},
    constants: {},
    errors: {},
    rpc: [],
  };
  for (const contract of [plugin, etl, browser]) {
    const { rpc, ...req } = structuredClone(contract.requirements);
    for (const [name, value] of Object.entries(req))
      result[name] = { ...result[name], ...value };
    result.rpc.push(...rpc);
  }
  return result;
}
test("all existing consumer contracts accept the supported surface", () => {
  for (const contract of [plugin, etl, browser])
    assert.equal(checkContract(contract, surface()).passed, true);
});
test("unrelated APIs, named struct fields and enum variants may be added", () => {
  const actual = surface();
  actual.calls["unrelated.newCall"] = { fields: [] };
  actual.storage["system.account"].value.struct.data.struct.newField = "u32";
  actual.calls["balances.transferKeepAlive"].fields[0].type.variant.NewAddress =
    { tuple: ["Bytes"] };
  for (const contract of [plugin, etl, browser])
    assert.equal(checkContract(contract, actual).passed, true);
});
test("removed calls identify the affected consumers", () => {
  const actual = surface();
  delete actual.calls["likes.like"];
  assert.match(
    checkContract(plugin, actual).failures.join("\n"),
    /calls.likes.like/,
  );
  assert.equal(checkContract(browser, actual).passed, false);
  assert.equal(checkContract(etl, actual).passed, true);
});
test("event payload changes identify the field and ETL dependency", () => {
  const actual = surface();
  actual.events["owners.UrlOwnerRegistered"].fields[1].type = "u64";
  assert.match(
    checkContract(etl, actual).failures.join("\n"),
    /events.owners.UrlOwnerRegistered.fields\[1\].type/,
  );
  assert.equal(checkContract(plugin, actual).passed, true);
});
test("reordered positional call fields are incompatible even when types are identical", () => {
  const actual = surface();
  actual.calls["likes.like"].fields.reverse();
  assert.equal(checkContract(plugin, actual).passed, false);
});
test("storage absence behavior and tuple widths are checked", () => {
  const actual = surface();
  actual.storage["likes.urls"].modifier = "Default";
  assert.equal(checkContract(plugin, actual).passed, false);
  actual.storage["likes.urls"].modifier = "Optional";
  actual.storage["likes.likes"].value.tuple[1] = "u64";
  assert.match(
    checkContract(browser, actual).failures.join("\n"),
    /storage.likes.likes.value.tuple\[1\]/,
  );
});
test("hashing changes break private wallet lookups but not metadata-driven plugin queries", () => {
  const actual = surface();
  actual.storage["likes.urls"].hashers = ["Twox64Concat"];
  assert.match(
    checkContract(browser, actual).failures.join("\n"),
    /storage.likes.urls.hashers/,
  );
  assert.equal(checkContract(plugin, actual).passed, true);
});
test("RPC removals, URL limits and signing changes fail fast", () => {
  const actual = surface();
  actual.rpc = actual.rpc.filter((method) => method !== "state_getKeysPaged");
  assert.match(
    checkContract(browser, actual).failures.join("\n"),
    /rpc.state_getKeysPaged/,
  );
  actual.constants["likes.maxLengthURL"].value = 128;
  assert.match(
    checkContract(plugin, actual).failures.join("\n"),
    /constants.likes.maxLengthURL.value/,
  );
  actual.extrinsic.signedExtensions.push("UnknownExtension");
  assert.match(
    checkContract(browser, actual).failures.join("\n"),
    /extrinsic.signedExtensions/,
  );
});
test("named account fields remain required", () => {
  const actual = surface();
  delete actual.storage["system.account"].value.struct.data.struct.free;
  assert.match(
    checkContract(plugin, actual).failures.join("\n"),
    /system.account.value.struct.data.struct.free/,
  );
});
test("the preserved baseline still fails when the current consumer contract is loosened", () => {
  const actual = surface();
  delete actual.calls["likes.like"];
  const loosened = structuredClone(plugin);
  delete loosened.requirements.calls;
  assert.equal(checkContract(loosened, actual).passed, true);
  assert.equal(checkContract(plugin, actual).passed, false);
});
test("malformed or empty contracts fail closed", () => {
  assert.throws(() => validateContract({}), /schemaVersion/);
  assert.throws(
    () => validateContract({ ...plugin, requirements: { callz: {} } }),
    /Unknown requirements/,
  );
  assert.throws(
    () => validateContract({ ...plugin, requirements: {} }),
    /empty/,
  );
  assert.throws(
    () =>
      validateContract({
        ...plugin,
        requirements: { calls: { "likes.like": {} } },
      }),
    /explicit shape/,
  );
});
test("portable type ID renumbering and transparent aliases do not cause false alarms", () => {
  const registry = new TypeRegistry();
  const make = (ids) => {
    const types = new Map([
      [
        ids[0],
        registry.createType("SiType", {
          path: [],
          params: [],
          def: { Primitive: "U32" },
          docs: [],
        }),
      ],
      [
        ids[1],
        registry.createType("SiType", {
          path: [],
          params: [],
          def: { Tuple: [ids[0]] },
          docs: [],
        }),
      ],
      [
        ids[2],
        registry.createType("SiType", {
          path: ["SomeNewtype"],
          params: [],
          def: { Composite: { fields: [{ type: ids[1] }] } },
          docs: [],
        }),
      ],
    ]);
    return { getSiType: (id) => types.get(id) };
  };
  assert.deepEqual(describeType(make([1, 2, 3]), 3), { tuple: ["u32"] });
  assert.deepEqual(describeType(make([21, 72, 99]), 99), { tuple: ["u32"] });
});
test("timeouts reject instead of leaving a green incomplete command", async () => {
  await assert.rejects(
    command([process.execPath, "-e", "setInterval(() => {}, 1000)"], {
      timeoutMs: 50,
    }),
    /timed out/,
  );
});

test("unrequired metadata types are not inspected or allowed to break consumers", async () => {
  const api = {
    registry: {
      lookup: {
        getSiType() {
          throw new Error("Do not resolve irrelevant types");
        },
      },
      chainTokens: ["WIKA"],
      chainDecimals: [12],
      chainSS58: 42,
      signedExtensions: [],
    },
    extrinsicVersion: 4,
    runtimeMetadata: {
      asLatest: {
        pallets: [
          {
            name: "Irrelevant",
            calls: { isSome: true, unwrap: () => ({ type: 999 }) },
            storage: { isSome: false },
            constants: [],
          },
        ],
      },
    },
    rpc: { rpc: { methods: async () => ({ methods: [] }) } },
  };
  const actual = await readSurface(api, { calls: { "likes.like": {} } });
  assert.deepEqual(actual.calls, {});
});

test('exhaustive financial event sets reject additions requiring consumer review', () => {
  const contract = { schemaVersion: 1, consumer: 'etl', description: 'Financial replay', behavior: ['Replay'], requirements: { events: { 'owners.Accrued': { fields: [] } }, exhaustiveEvents: ['owners'] } };
  assert.equal(checkContract(contract, { events: { 'owners.Accrued': { fields: [] } }, rpc: [] }).passed, true);
  const changed = checkContract(contract, { events: { 'owners.Accrued': { fields: [] }, 'owners.Refunded': { fields: [] } }, rpc: [] });
  assert.equal(changed.passed, false);
  assert.match(changed.failures.join('\n'), /complete event set requires consumer review/);
  assert.throws(() => validateContract({ ...contract, requirements: { ...contract.requirements, exhaustiveEvents: ['balances'] } }), /exhaustiveEvents/);
});
