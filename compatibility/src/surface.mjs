import { stringCamelCase } from "@polkadot/util";
const lowerFirst = stringCamelCase;

/** Resolve portable type IDs to stable structural shapes, never serializing Lookup123 names. */
export function describeType(lookup, id, ancestors = new Set()) {
  const number = Number(id.toString());
  const type = lookup.getSiType(number);
  const path = type.path.map((v) => v.toString());
  const name = path.at(-1);
  if (
    [
      "AccountId32",
      "H256",
      "H160",
      "DispatchError",
      "DispatchInfo",
      "Phase",
    ].includes(name)
  )
    return name;
  if (ancestors.has(number))
    return { recursive: path.join("::") || "anonymous" };
  const next = new Set([...ancestors, number]);
  const desc = (child) => describeType(lookup, child, next);
  const def = type.def;
  if (def.isPrimitive) return lowerFirst(def.asPrimitive.type);
  if (def.isSequence) {
    const inner = desc(def.asSequence.type);
    return inner === "u8" ? "Bytes" : { sequence: inner };
  }
  if (def.isArray)
    return {
      array: {
        length: def.asArray.len.toNumber(),
        type: desc(def.asArray.type),
      },
    };
  if (def.isTuple) return { tuple: def.asTuple.map(desc) };
  if (def.isCompact) return { compact: desc(def.asCompact.type) };
  if (def.isBitSequence)
    return {
      bitSequence: {
        store: desc(def.asBitSequence.bitStoreType),
        order: desc(def.asBitSequence.bitOrderType),
      },
    };
  if (def.isVariant) {
    if (name === "Option")
      return {
        option: desc(
          def.asVariant.variants.find((v) => v.name.toString() === "Some")
            .fields[0].type,
        ),
      };
    return {
      variant: Object.fromEntries(
        def.asVariant.variants.map((v) => [
          v.name.toString(),
          fieldsShape(v.fields, desc),
        ]),
      ),
    };
  }
  if (def.isComposite) {
    const fields = def.asComposite.fields;
    // Transparent newtypes (e.g. BoundedVec<u8, _>) preserve their wire representation.
    if (fields.length === 1 && fields[0].name.isNone)
      return desc(fields[0].type);
    return fieldsShape(fields, desc);
  }
  throw new Error(`Unsupported metadata type ${number} (${path.join("::")})`);
}
function fieldsShape(fields, desc) {
  if (fields.every((field) => field.name.isSome))
    return {
      struct: Object.fromEntries(
        fields.map((f) => [f.name.unwrap().toString(), desc(f.type)]),
      ),
    };
  return { tuple: fields.map((f) => desc(f.type)) };
}
function fieldsList(fields, lookup) {
  return fields.map((field) => ({
    name: field.name.isSome ? field.name.unwrap().toString() : null,
    type: describeType(lookup, field.type),
  }));
}
export async function readSurface(api, requirements) {
  const needs = (category, selector) =>
    !requirements || (category === "events" && requirements.exhaustiveEvents?.includes(selector.split(".")[0])) || Object.hasOwn(requirements[category] ?? {}, selector);
  const needsSection = (category, section) =>
    !requirements ||
    Object.keys(requirements[category] ?? {}).some((selector) =>
      selector.startsWith(`${section}.`),
    );
  const lookup = api.registry.lookup;
  const metadata = api.runtimeMetadata.asLatest;
  const surface = {
    calls: {},
    events: {},
    storage: {},
    constants: {},
    errors: {},
    rpc: [],
    properties: {},
    extrinsic: {},
  };
  for (const pallet of metadata.pallets) {
    const section = lowerFirst(pallet.name.toString());
    for (const [kind, target] of [
      ["calls", "calls"],
      ["events", "events"],
      ["errors", "errors"],
    ]) {
      const entry = pallet[kind];
      if (!entry?.isSome || !needsSection(target, section)) continue;
      const variants = lookup.getSiType(entry.unwrap().type).def.asVariant
        .variants;
      for (const variant of variants) {
        const method =
          kind === "calls"
            ? variant.name
                .toString()
                .replace(/_([a-z])/g, (_, c) => c.toUpperCase())
            : variant.name.toString();
        if (!needs(target, `${section}.${method}`)) continue;
        surface[target][`${section}.${method}`] = {
          fields: fieldsList(variant.fields, lookup),
        };
      }
    }
    if (pallet.storage.isSome) {
      for (const entry of pallet.storage.unwrap().items) {
        const selector = `${section}.${lowerFirst(entry.name.toString())}`;
        if (!needs("storage", selector)) continue;
        const type = entry.type;
        surface.storage[selector] = {
          modifier: entry.modifier.toString(),
          keys: type.isMap
            ? type.asMap.hashers.length === 1
              ? [describeType(lookup, type.asMap.key)]
              : lookup
                  .getSiType(type.asMap.key)
                  .def.asTuple.map((id) => describeType(lookup, id))
            : [],
          hashers: type.isMap
            ? type.asMap.hashers.map((h) => h.toString())
            : [],
          value: describeType(
            lookup,
            type.isMap ? type.asMap.value : type.asPlain,
          ),
        };
      }
    }
    for (const entry of pallet.constants) {
      const method = lowerFirst(entry.name.toString());
      if (!needs("constants", `${section}.${method}`)) continue;
      surface.constants[`${section}.${method}`] = {
        type: describeType(lookup, entry.type),
        value: api.consts[section][method].toJSON(),
      };
    }
  }
  surface.rpc = (await api.rpc.rpc.methods()).methods
    .map((m) => m.toString())
    .sort();
  surface.properties = {
    symbol: api.registry.chainTokens[0],
    decimals: api.registry.chainDecimals[0],
    ss58Prefix: api.registry.chainSS58,
  };
  surface.extrinsic = {
    version: api.extrinsicVersion,
    signedExtensions: api.registry.signedExtensions,
  };
  return surface;
}
