import { test } from "node:test";
import { assertEquals, assertThrows } from "@std/assert";
import { DEV_RATES } from "../rates.ts";
import {
  findScenario,
  HUB_OWNER_BOOK,
  HUB_SCHEME,
  hubConfig,
  invalid,
  invalidHub,
  joinsHub,
  ledgerNetwork,
  live,
  MARKET_MAKER_PARKED,
  MAX_NODES,
  micros,
  minted,
  OWNER_BOOK,
  peripheryOf,
  poolDrops,
  resumes,
  type ScenarioDef,
  SCENARIOS,
  settlesOnXrpl,
  startsFresh,
  TOKEN_SCHEMES,
  unavailable,
  UNLIMITED_STUB,
  validates,
  validatorFile,
  XRPL_SCHEMES,
} from "./scenarios.ts";

test("every scenario's periphery nodes are pinned to its core nodes", () => {
  for (const def of SCENARIOS) {
    assertEquals(invalid(def), undefined, def.id);
  }
  const base: ScenarioDef = {
    id: "t",
    name: "t",
    description: "t",
    schemes: [["stub"], ["stub"], ["stub"]],
    addedSchemes: ["stub"],
    bootstrap: [0],
  };
  const pinned = (periphery: ScenarioDef["periphery"]) =>
    invalid({ ...base, periphery });
  assertEquals(pinned([{ node: 1, uplinks: [0] }]), undefined);
  assertEquals(pinned([{ node: 3, uplinks: [0] }]), "no node 3");
  assertEquals(
    pinned([{ node: 0, uplinks: [1] }]),
    "periphery node 0 is a bootstrap node",
  );
  assertEquals(
    pinned([{ node: 1, uplinks: [] }]),
    "periphery node 1 has no uplink",
  );
  assertEquals(
    pinned([{ node: 1, uplinks: [0, 0] }]),
    "periphery node 1 names an uplink twice",
  );
  assertEquals(
    pinned([{ node: 1, uplinks: [2] }, { node: 2, uplinks: [0] }]),
    "periphery node 1 is pinned to 2, no core node",
  );
  assertEquals(
    pinned([{ node: 1, uplinks: [0] }, { node: 1, uplinks: [0] }]),
    "a periphery node twice",
  );
});

// spec/areas/dev.md#scenarios: the market maker is parked until it runs on
// hubs, so its scenarios cannot run, even with the fake ledger, and say
// why; their layout stays as it was, for when it does.
test("the market maker's scenario: a core node and two pinned to it, parked", () => {
  const def = findScenario("market-maker")!;
  assertEquals(invalid(def), undefined);
  assertEquals(peripheryOf(def, 0), undefined);
  assertEquals(peripheryOf(def, 1)?.uplinks, [0]);
  assertEquals(peripheryOf(def, 2)?.uplinks, [0]);
  // Only the nodes that run DSUSD's scheme are minted it.
  assertEquals(def.schemes.map((schemes) => minted(def, schemes)), [
    true,
    false,
    true,
  ]);
  assertEquals(minted(def, def.addedSchemes), true);
  assertEquals(unavailable(def, false), MARKET_MAKER_PARKED);
  assertEquals(unavailable(def, true), MARKET_MAKER_PARKED);
  // Unparked, it needs the fake ledger's issuer and pool.
  const unparked = { ...def, unavailable: undefined };
  assertEquals(
    unavailable(unparked, false),
    "needs --xrpl fake: only the fake ledger's issuer mints DSUSD on demand",
  );
  assertEquals(
    unavailable({ ...unparked, mint: undefined }, false)?.includes("pool"),
    true,
  );
  assertEquals(unavailable(unparked, true), undefined);
});

test("the market maker's scenario with two targets: both nodes pinned to both, parked", () => {
  const def = findScenario("market-maker-two-targets")!;
  assertEquals(invalid(def), undefined);
  assertEquals(peripheryOf(def, 0), undefined);
  assertEquals(peripheryOf(def, 1), undefined);
  assertEquals(peripheryOf(def, 2)?.uplinks, [0, 1]);
  assertEquals(peripheryOf(def, 3)?.uplinks, [0, 1]);
  assertEquals(def.schemes.map((schemes) => minted(def, schemes)), [
    true,
    true,
    false,
    true,
  ]);
  assertEquals(unavailable(def, true), MARKET_MAKER_PARKED);
  assertEquals(
    unavailable({ ...def, unavailable: undefined }, true),
    undefined,
  );
});

// spec/roadmap.md#hubs-only: no scenario that runs settles directly. Each
// of its schemes is the stub with unlimited trust, which never settles, an
// XRP Ledger scheme as an owner book, which peers with nobody, or a hub's,
// whose ID names it so (spec/decisions/hub-scheme-names.md).
test("no scenario that runs settles directly", () => {
  const settlesNot = (def: ScenarioDef, scheme: string) =>
    scheme.startsWith("hub-") ||
    (scheme === "stub" && def.configs?.stub === UNLIMITED_STUB) ||
    (XRPL_SCHEMES.includes(scheme) && def.configs?.[scheme] === OWNER_BOOK);
  assertEquals(JSON.parse(UNLIMITED_STUB), { trust: "unlimited" });
  assertEquals(JSON.parse(OWNER_BOOK), { peers: false });
  for (const def of SCENARIOS) {
    if (def.unavailable !== undefined) continue;
    for (const schemes of [...def.schemes, def.addedSchemes]) {
      for (const scheme of schemes) {
        assertEquals(settlesNot(def, scheme), true, `${def.id} ${scheme}`);
      }
    }
  }
  // Only the market maker's are parked.
  assertEquals(
    SCENARIOS.filter((def) => def.unavailable !== undefined).map((def) =>
      def.id
    ),
    ["market-maker", "market-maker-two-targets"],
  );
  const ids = SCENARIOS.map((def) => def.id);
  assertEquals(new Set(ids).size, ids.length, "an ID twice");
});

test("a pool is priced at the dev rates, exactly", () => {
  assertEquals(micros("0.92"), 920_000n);
  assertEquals(micros("50000"), 50_000_000_000n);
  assertEquals(micros("1.0000001"), undefined);
  assertEquals(micros("-1"), undefined);
  assertEquals(micros("1e3"), undefined);
  // A dollar buys two XRP: 50000 dollars are 100000 XRP.
  assertEquals(poolDrops("50000", DEV_RATES.XRP), 100_000_000_000n);
  assertEquals(poolDrops("0.5", "0.92"), 460_000n);
  // Below a drop rounds down.
  assertEquals(poolDrops("0.000001", "0.5"), 0n);
  assertThrows(() => poolDrops("lots", "2"), Error, "no pool");
});

// spec/decisions/mainnet-schemes.md: a node runs only its realm's schemes,
// so a live scenario's nodes run only mainnet's, and a test scenario's none
// of them; its token is its network's dollar. The live realm runs only
// against the fake ledger's mainnet, and a live scenario's nodes start from
// scratch each run, since mainnet never resets.
test("a live scenario runs mainnet's schemes on the fake ledger's mainnet only", () => {
  const mainnet = ["xrpl-xrp", "xrpl-rlusd"];
  const ids = SCENARIOS.filter(live).map((def) => def.id);
  assertEquals(ids, ["live-owner-books"]);
  for (const def of SCENARIOS) {
    for (const schemes of [...def.schemes, def.addedSchemes]) {
      for (const scheme of schemes) {
        if (scheme === "stub") continue;
        assertEquals(
          mainnet.includes(scheme),
          live(def),
          `${def.id} ${scheme}`,
        );
      }
    }
    for (const token of [def.mint?.token, def.pool?.token]) {
      if (token === undefined) continue;
      assertEquals(token === "RLUSD", live(def), def.id);
    }
    assertEquals(ledgerNetwork(def), live(def) ? "mainnet" : "testnet");
    assertEquals(resumes(def), !live(def) && def.hub === undefined, def.id);
  }
  // Its node holds XRP and RLUSD in owner books on one account, and is
  // minted RLUSD.
  const books = findScenario("live-owner-books")!;
  assertEquals(TOKEN_SCHEMES.RLUSD, "xrpl-rlusd");
  assertEquals(books.schemes, [["xrpl-xrp", "xrpl-rlusd"]]);
  assertEquals(books.schemes.map((schemes) => minted(books, schemes)), [
    true,
  ]);
  assertEquals(
    unavailable(books, false),
    "needs --xrpl fake: the live realm runs only against the fake ledger's mainnet, never with real money",
  );
  assertEquals(unavailable(books, true), undefined);
  assertEquals(
    unavailable({ ...books, mint: undefined, pool: undefined }, false),
    "needs --xrpl fake: the live realm runs only against the fake ledger's mainnet, never with real money",
  );
});

// spec/areas/dev.md#scenarios: DSUSD's scenario is its owner book, which
// peers with nobody, minted by the fake ledger's issuer beside its pool.
test("the DSUSD owner book: minted, beside its pool, on the fake ledger only", () => {
  const def = findScenario("dsusd-owner-book")!;
  assertEquals(invalid(def), undefined);
  assertEquals(def.schemes, [[TOKEN_SCHEMES.DSUSD]]);
  assertEquals(minted(def, def.schemes[0]), true);
  assertEquals(def.pool?.token, "DSUSD");
  assertEquals(resumes(def), true);
  assertEquals(
    unavailable(def, false),
    "needs --xrpl fake: only the fake ledger's issuer mints DSUSD on demand",
  );
  assertEquals(unavailable(def, true), undefined);
});

// spec/areas/dev.md#scenarios: dev's scenarios that are not about
// settlement run the stub with unlimited trust, which never settles.
test("the stub's scenarios run it with unlimited trust", () => {
  for (const id of ["two-nodes", "six-nodes", "one-node"]) {
    const def = findScenario(id)!;
    assertEquals(def.configs, { stub: UNLIMITED_STUB }, id);
    assertEquals(
      [...def.schemes, def.addedSchemes].flat().every((s) => s === "stub"),
      true,
      id,
    );
    assertEquals(settlesOnXrpl(def), false, id);
    assertEquals(resumes(def), true, id);
    assertEquals(unavailable(def, false), undefined, id);
  }
});

// spec/areas/dev.md#scenarios: across currencies, A has dollars on the
// unlimited stub, C has XRP in a hub four validators found, and B has both
// and converts; A and B meet on the stub, B and C over the hub alone.
test("cross-currency: the unlimited stub, a hub, and a node in both", () => {
  const def = findScenario("cross-currency")!;
  assertEquals(invalid(def), undefined);
  assertEquals(def.schemes.slice(0, 3), [
    ["stub"],
    ["stub", HUB_OWNER_BOOK, HUB_SCHEME],
    [HUB_OWNER_BOOK, HUB_SCHEME],
  ]);
  assertEquals(def.configs, {
    stub: UNLIMITED_STUB,
    [HUB_OWNER_BOOK]: OWNER_BOOK,
  });
  assertEquals(
    def.schemes.map((_, index) => validates(def, index)),
    [false, false, false, true, true, true, true],
  );
  assertEquals(
    def.schemes.map((_, index) => joinsHub(def, index)),
    [false, true, true, false, false, false, false],
  );
  assertEquals(def.schemes[def.hub!.sponsor], [HUB_OWNER_BOOK]);
  assertEquals(joinsHub(def, def.schemes.length), true, "an added node");
  assertEquals(resumes(def), false);
  assertEquals(
    unavailable(def, false),
    "needs --xrpl fake: each run founds a hub of its own, whose escrow stays off the public testnet",
  );
  assertEquals(unavailable(def, true), undefined);
});

// spec/areas/xrpl.md#the-peg-guard: a dollar token's guard fails closed
// without a price, which dev timers make seconds, so every scenario whose
// nodes settle in a token makes that token's pool, priced at the dev rates.
test("every scenario that settles in a dollar token has its pool", () => {
  for (const def of SCENARIOS) {
    const schemes = [...def.schemes, def.addedSchemes].flat();
    for (const [token, scheme] of Object.entries(TOKEN_SCHEMES)) {
      if (!schemes.includes(scheme)) continue;
      assertEquals(def.pool?.token, token, `${def.id} has no ${token} pool`);
    }
  }
});

// spec/areas/dev.md#scenarios: the hub scenario's four validators run the
// hub the first of them founds; its two members, and nodes added later,
// join it with their owner book of XRP, which peers with nobody, so they
// peer over the hub alone; only the fake ledger has it.
test("the hub scenario: four validators, a sponsor among them, two members", () => {
  const def = findScenario("hub")!;
  assertEquals(invalid(def), undefined);
  assertEquals(def.schemes.length <= MAX_NODES, true);
  assertEquals(
    def.schemes.map((_, index) => validates(def, index)),
    [true, true, true, true, false, false],
  );
  assertEquals(
    def.schemes.map((_, index) => joinsHub(def, index)),
    [false, false, false, false, true, true],
  );
  assertEquals(joinsHub(def, def.schemes.length), true, "an added node");
  assertEquals(validates(def, def.schemes.length), false, "an added node");
  assertEquals(def.hub?.sponsor, 0);
  assertEquals(def.schemes[0], [HUB_OWNER_BOOK]);
  // Members are its bootstrap nodes: they meet without the validators.
  assertEquals(def.bootstrap, def.hub?.members);
  assertEquals(settlesOnXrpl(def), true);
  // A restart starts it from scratch: the fake ledger is a new chain, and
  // its hub's escrow is on the old one.
  assertEquals(resumes(def), false);
  assertEquals(
    startsFresh(def),
    "founds its hub on a new fake ledger, since its escrow stays on the old one",
  );
  assertEquals(startsFresh(findScenario("two-nodes")!), undefined);
  assertEquals(
    unavailable(def, false),
    "needs --xrpl fake: each run founds a hub of its own, whose escrow stays off the public testnet",
  );
  assertEquals(unavailable(def, true), undefined);
  // No node of a scenario without a hub runs the hub's scheme, whose
  // config only a founding gives.
  for (const other of SCENARIOS) {
    if (other.hub !== undefined) continue;
    assertEquals(
      [...other.schemes, other.addedSchemes].flat().includes(HUB_SCHEME),
      false,
      other.id,
    );
    assertEquals(joinsHub(other, 0), false, other.id);
    assertEquals(validates(other, 0), false, other.id);
  }
});

test("a hub's validator file and its members' config", () => {
  assertEquals(JSON.parse(validatorFile("abc", "ws://127.0.0.1:1")), {
    sponsors: ["abc"],
    hubs: [],
    xrpl: "ws://127.0.0.1:1",
    network: "testnet",
  });
  const publishers = { publishers: [{ key: "ED00", sites: ["https://x"] }] };
  assertEquals(
    JSON.parse(validatorFile("abc", "ws://127.0.0.1:1", publishers)),
    {
      sponsors: ["abc"],
      hubs: [],
      xrpl: "ws://127.0.0.1:1",
      network: "testnet",
      ...publishers,
    },
  );
  assertEquals(JSON.parse(hubConfig("00ff")), { rules: "00ff" });
});

test("a hub's nodes are checked before the scenario starts", () => {
  const book = [HUB_OWNER_BOOK];
  const member = [HUB_OWNER_BOOK, HUB_SCHEME];
  const base: ScenarioDef = {
    id: "t",
    name: "t",
    description: "t",
    schemes: [book, [], [], [], member, member],
    addedSchemes: member,
    bootstrap: [4, 5],
    configs: { [HUB_OWNER_BOOK]: `{"peers": false}` },
    hub: { validators: [0, 1, 2, 3], sponsor: 0, members: [4, 5] },
  };
  assertEquals(invalidHub(base), undefined);
  assertEquals(invalidHub({ ...base, hub: undefined }), undefined);
  const hub = (changes: Partial<NonNullable<ScenarioDef["hub"]>>) =>
    invalid({ ...base, hub: { ...base.hub!, ...changes } });
  assertEquals(
    hub({ validators: [0, 1, 2] }),
    "a hub has 4 to 16 validators, not 3",
  );
  assertEquals(
    hub({ validators: Array.from({ length: 17 }, (_, i) => i) }),
    "a hub has 4 to 16 validators, not 17",
  );
  assertEquals(hub({ validators: [0, 1, 2, 2] }), "a hub validator twice");
  assertEquals(hub({ validators: [0, 1, 2, 6] }), "no node 6");
  assertEquals(hub({ members: [4, 4] }), "a hub member twice");
  assertEquals(hub({ members: [4, 7] }), "no node 7");
  assertEquals(hub({ members: [] }), "a hub has no member");
  assertEquals(hub({ sponsor: 9 }), "no node 9");
  assertEquals(
    hub({ sponsor: 1 }),
    "the sponsor runs no xrpl-testnet-xrp, which funds the escrow",
  );
  // A validator may be a member too, as long as it runs what members run.
  assertEquals(
    invalid({
      ...base,
      schemes: [member, [], [], [], member, member],
      hub: { ...base.hub!, members: [0, 4, 5] },
    }),
    undefined,
  );
  const schemes = (index: number, these: string[]) =>
    invalid({
      ...base,
      schemes: base.schemes.map((s, i) => i === index ? these : s),
    });
  assertEquals(
    schemes(4, [HUB_SCHEME]),
    "hub member 4 runs hub-xrpl-testnet-xrp, not hub-xrpl-testnet-xrp and xrpl-testnet-xrp",
  );
  // A member may run another scheme, as long as no other member does: two
  // that share one would peer over it before they joined the hub.
  assertEquals(schemes(5, [...member, "stub"]), undefined);
  assertEquals(
    invalid({
      ...base,
      schemes: [book, [], [], [], [...member, "stub"], ["stub", ...member]],
    }),
    "hub members 4 and 5 both run stub, so they would peer over it",
  );
  assertEquals(
    schemes(1, member),
    "node 1 runs hub-xrpl-testnet-xrp and is no hub member",
  );
  assertEquals(
    invalid({ ...base, addedSchemes: ["stub"] }),
    "added nodes run stub, not hub-xrpl-testnet-xrp and xrpl-testnet-xrp alone",
  );
  const configs = (these: Record<string, string> | undefined) =>
    invalid({ ...base, configs: these });
  const peers = `xrpl-testnet-xrp peers: its config must say "peers": false`;
  assertEquals(configs(undefined), peers);
  assertEquals(configs({ [HUB_OWNER_BOOK]: "" }), peers);
  assertEquals(configs({ [HUB_OWNER_BOOK]: `{"peers": true}` }), peers);
  assertEquals(configs({ [HUB_OWNER_BOOK]: "{" }), peers);
  assertEquals(
    configs({ ...base.configs, [HUB_SCHEME]: hubConfig("00") }),
    "hub-xrpl-testnet-xrp's config is the founded hub's rules",
  );
  // The periphery's checks come first.
  assertEquals(
    invalid({ ...base, periphery: [{ node: 9, uplinks: [0] }] }),
    "no node 9",
  );
});
