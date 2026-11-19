/** Compare only required surfaces. Extra object fields/enum variants are compatible. */
export function differences(expected, actual, path = "") {
  if (expected === "*") return [];
  if (Array.isArray(expected)) {
    if (!Array.isArray(actual) || expected.length !== actual.length)
      return [
        `${path}: expected ${JSON.stringify(expected)}, received ${JSON.stringify(actual)}`,
      ];
    return expected.flatMap((item, i) =>
      differences(item, actual[i], `${path}[${i}]`),
    );
  }
  if (expected && typeof expected === "object") {
    if (!actual || typeof actual !== "object")
      return [
        `${path}: required ${JSON.stringify(expected)}, received ${JSON.stringify(actual)}`,
      ];
    return Object.entries(expected).flatMap(([key, value]) =>
      differences(value, actual[key], path ? `${path}.${key}` : key),
    );
  }
  return expected === actual
    ? []
    : [
        `${path}: expected ${JSON.stringify(expected)}, received ${JSON.stringify(actual) ?? "missing"}`,
      ];
}

const sections = ["calls", "storage", "events", "constants", "errors"];
/** Fail closed on typos/empty contracts rather than accidentally running no checks. */
export function validateContract(contract) {
  const allowed = new Set([
    "schemaVersion",
    "consumer",
    "description",
    "requirements",
    "behavior",
  ]);
  if (!contract || typeof contract !== "object" || Array.isArray(contract))
    throw new Error("Contract must be an object");
  for (const key of Object.keys(contract))
    if (!allowed.has(key)) throw new Error(`Unknown contract key: ${key}`);
  if (
    contract.schemaVersion !== 1 ||
    !contract.consumer ||
    !contract.description
  )
    throw new Error(
      "Contract needs schemaVersion: 1, consumer, and description",
    );
  if (!Array.isArray(contract.behavior) || contract.behavior.length === 0)
    throw new Error("Contract must describe its behavior checks");
  const req = contract.requirements;
  if (
    !req ||
    typeof req !== "object" ||
    Array.isArray(req) ||
    !Object.keys(req).length
  )
    throw new Error("Contract requirements must not be empty");
  const permitted = new Set([...sections, "rpc", "properties", "extrinsic", "exhaustiveEvents"]);
  for (const key of Object.keys(req))
    if (!permitted.has(key))
      throw new Error(`Unknown requirements key: ${key}`);
  if (req.exhaustiveEvents !== undefined && (!Array.isArray(req.exhaustiveEvents) || !req.exhaustiveEvents.length || req.exhaustiveEvents.some(v => typeof v !== "string" || !/^\w+$/.test(v) || !Object.keys(req.events ?? {}).some(key => key.startsWith(v + ".")))))
    throw new Error("exhaustiveEvents must list pallets with declared event requirements");
  let checks = 0;
  for (const key of sections) {
    if (!req[key]) continue;
    if (typeof req[key] !== "object" || Array.isArray(req[key]))
      throw new Error(`${key} must be an object`);
    for (const [selector, shape] of Object.entries(req[key])) {
      if (!/^\w+\.\w+$/.test(selector))
        throw new Error(`Invalid ${key} selector: ${selector}`);
      if (
        !shape ||
        typeof shape !== "object" ||
        Array.isArray(shape) ||
        !Object.keys(shape).length
      )
        throw new Error(`${key}.${selector} needs an explicit shape`);
      checks++;
    }
  }
  if (
    req.rpc &&
    (!Array.isArray(req.rpc) || req.rpc.some((v) => typeof v !== "string"))
  )
    throw new Error("rpc must be a list of method names");
  checks +=
    (req.rpc?.length ?? 0) +
    Object.keys(req.properties ?? {}).length +
    Object.keys(req.extrinsic ?? {}).length;
  if (!checks) throw new Error("Contract declares no checks");
  return contract;
}
export function checkContract(contract, surface) {
  validateContract(contract);
  const { rpc, exhaustiveEvents, ...required } = contract.requirements;
  const failures = differences(required, surface);
  for (const pallet of exhaustiveEvents ?? []) {
    const expected = Object.keys(required.events ?? {}).filter(key => key.startsWith(pallet + ".")).sort();
    const actual = Object.keys(surface.events ?? {}).filter(key => key.startsWith(pallet + ".")).sort();
    failures.push(...differences(expected, actual, `events.${pallet}: complete event set requires consumer review`));
  }
  for (const method of rpc ?? [])
    if (!surface.rpc.includes(method))
      failures.push(`rpc.${method}: required method is missing`);
  return {
    consumer: contract.consumer,
    passed: failures.length === 0,
    failures,
  };
}
