import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import {
  hubLedger,
  hubOwnerBook,
  hubOwnerLedger,
  hubScheme,
  hubsOf,
  hubTerms,
  invalidHubs,
  type ScenarioHub,
  sponsors,
  validatesAny,
  xrpOwnerBook,
} from "./hubs.ts";
import {
  findScenario,
  HUB_LEDGER,
  HUB_OWNER_BOOK,
  HUB_SCHEME,
  hubConfig,
  type ScenarioDef,
} from "./scenarios.ts";

const DSUSD_BOOK = "xrpl-testnet-dsusd";
const DSUSD_HUB = "hub-xrpl-testnet-dsusd";

// spec/decisions/hub-scheme-names.md: each hub's scheme, ledger and owner
// book follow from its asset; a DSUSD hub's terms name the token as the
// XRP Ledger writes it, and an XRP hub's are the starting ones.
test("a hub's scheme, ledgers and terms follow from its asset", () => {
  const xrp: ScenarioHub = { validators: [0], sponsor: 0, members: [1] };
  const dsusd: ScenarioHub = { ...xrp, token: "DSUSD" };
  assertEquals(
    [hubScheme(xrp), hubLedger(xrp), hubOwnerBook(xrp), hubOwnerLedger(xrp)],
    [HUB_SCHEME, HUB_LEDGER, HUB_OWNER_BOOK, "xrpl-testnet-xrp+xrp"],
  );
  assertEquals(
    [
      hubScheme(dsusd),
      hubLedger(dsusd),
      hubOwnerBook(dsusd),
      hubOwnerLedger(dsusd),
    ],
    [DSUSD_HUB, "hub-xrpl-testnet-dsusd+usd", DSUSD_BOOK, `${DSUSD_BOOK}+usd`],
  );
  assertEquals(hubTerms(xrp), "");
  assertEquals(JSON.parse(hubTerms(dsusd)), {
    asset: {
      currency: "4453555344000000000000000000000000000000",
      issuer: "rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M",
    },
  });
  // The one hub of the `hub` scenario is its hubs, on the testnet; the
  // same ones each time, so that they compare as themselves.
  const hub = findScenario("hub")!;
  assertEquals(hubsOf(hub), [{ ...hub.hub!, network: "testnet" }]);
  assert(hubsOf(hub)[0] === hubsOf(hub)[0]);
  assertEquals(hubsOf(findScenario("two-nodes")!), []);
});

// spec/decisions/hub-scheme-names.md: on mainnet, a live scenario's, the
// XRP hub's scheme, ledgers and owner book are mainnet's, and an RLUSD
// hub's terms name RLUSD by its issuer on mainnet.
test("a mainnet hub's scheme, ledgers and terms follow from its asset", () => {
  const xrp: ScenarioHub = {
    validators: [0],
    sponsor: 0,
    members: [1],
    network: "mainnet",
  };
  const rlusd: ScenarioHub = { ...xrp, token: "RLUSD" };
  assertEquals(
    [hubScheme(xrp), hubLedger(xrp), hubOwnerBook(xrp), hubOwnerLedger(xrp)],
    ["hub-xrpl-xrp", "hub-xrpl-xrp+xrp", "xrpl-xrp", "xrpl-xrp+xrp"],
  );
  assertEquals(
    [
      hubScheme(rlusd),
      hubLedger(rlusd),
      hubOwnerBook(rlusd),
      hubOwnerLedger(rlusd),
    ],
    ["hub-xrpl-rlusd", "hub-xrpl-rlusd+usd", "xrpl-rlusd", "xrpl-rlusd+usd"],
  );
  assertEquals(hubTerms(xrp), "");
  assertEquals(JSON.parse(hubTerms(rlusd)), {
    asset: {
      currency: "524C555344000000000000000000000000000000",
      issuer: "rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De",
    },
  });
  const def = findScenario("live-hub")!;
  assertEquals(hubsOf(def).map((hub) => hub.network), ["mainnet", "mainnet"]);
  assertEquals(xrpOwnerBook(def), "xrpl-xrp");
  assertEquals(xrpOwnerBook(findScenario("hub")!), HUB_OWNER_BOOK);
  // A sponsor in the live realm funds the escrow from its mainnet XRP, and
  // the fake mainnet has no DSUSD.
  assertEquals(
    invalidHubs({
      ...def,
      schemes: def.schemes.map((s, i) => i === 0 ? [HUB_OWNER_BOOK] : s),
    }),
    "the sponsor runs no xrpl-xrp, which funds the escrow",
  );
  assertEquals(
    invalidHubs({
      ...def,
      hubs: [def.hubs![0], { ...def.hubs![1], token: "DSUSD" }],
    }),
    "no hub of DSUSD on the fake mainnet",
  );
});

test("several hubs are checked before the scenario starts", () => {
  const xrpMember = [HUB_OWNER_BOOK, HUB_SCHEME];
  const dsusdMember = [HUB_OWNER_BOOK, DSUSD_BOOK, DSUSD_HUB];
  const both = [...xrpMember, DSUSD_BOOK, DSUSD_HUB];
  const validators = [0, 1, 2, 3];
  const base: ScenarioDef = {
    id: "t",
    name: "t",
    description: "t",
    schemes: [[HUB_OWNER_BOOK], [], [], [], xrpMember, both, dsusdMember],
    addedSchemes: xrpMember,
    bootstrap: [5],
    mint: { token: "DSUSD", value: "100" },
    pool: { token: "DSUSD", value: "50000", tradingFee: 500 },
    hubs: [
      { validators, sponsor: 0, members: [4, 5] },
      { validators, sponsor: 0, members: [5, 6], token: "DSUSD" },
    ],
  };
  assertEquals(invalidHubs(base), undefined);
  assertEquals(invalidHubs({ ...base, hubs: undefined }), undefined);
  assertEquals(sponsors(base), [0]);
  assertEquals(validatesAny(base, 3), true);
  assertEquals(validatesAny(base, 4), false);
  const [xrp, dsusd] = base.hubs!;
  const hubs = (these: ScenarioDef["hubs"]) =>
    invalidHubs({ ...base, hubs: these });
  assertEquals(
    invalidHubs({ ...base, hub: xrp }),
    "a scenario has a hub or hubs, not both",
  );
  assertEquals(hubs([]), "no hub in hubs");
  assertEquals(hubs([xrp, { ...xrp, members: [5] }]), "two hubs of XRP");
  assertEquals(
    hubs([xrp, { ...dsusd, token: "RLUSD" }]),
    "no hub of RLUSD on the fake testnet",
  );
  assertEquals(
    invalidHubs({ ...base, mint: undefined }),
    "the DSUSD hub's members are minted no DSUSD",
  );
  assertEquals(
    invalidHubs({ ...base, pool: undefined }),
    "no pool prices DSUSD, whose guard halts its hub's ledger",
  );
  assertEquals(
    hubs([xrp, { ...dsusd, validators: [0, 1, 2] }]),
    "a hub has 4 to 16 validators, not 3",
  );
  assertEquals(
    hubs([{ ...xrp, members: [4, 4] }, dsusd]),
    "a hub member twice",
  );
  assertEquals(hubs([{ ...xrp, members: [4, 9] }, dsusd]), "no node 9");
  assertEquals(
    hubs([xrp, { ...dsusd, sponsor: 1 }]),
    "the sponsor runs no xrpl-testnet-xrp, which funds the escrow",
  );
  assertEquals(hubs([xrp, { ...dsusd, members: [] }]), "a hub has no member");
  const schemes = (index: number, these: string[]) =>
    invalidHubs({
      ...base,
      schemes: base.schemes.map((s, i) => i === index ? these : s),
    });
  // A member of the DSUSD hub runs the book of XRP too, for the escrow's
  // gas.
  assertEquals(
    schemes(6, [DSUSD_BOOK, DSUSD_HUB]),
    "hub member 6 runs xrpl-testnet-dsusd, hub-xrpl-testnet-dsusd, not hub-xrpl-testnet-dsusd, xrpl-testnet-dsusd, xrpl-testnet-xrp alone",
  );
  assertEquals(
    schemes(4, [...xrpMember, "stub"]),
    "hub member 4 runs xrpl-testnet-xrp, hub-xrpl-testnet-xrp, stub, not hub-xrpl-testnet-xrp, xrpl-testnet-xrp alone",
  );
  assertEquals(
    schemes(1, dsusdMember),
    "node 1 runs hub-xrpl-testnet-dsusd and is no member of its hub",
  );
  assertEquals(
    invalidHubs({ ...base, addedSchemes: ["stub"] }),
    "added nodes run stub, not a member's schemes alone",
  );
  assertEquals(
    invalidHubs({
      ...base,
      configs: { [DSUSD_HUB]: hubConfig("00") },
    }),
    "hub-xrpl-testnet-dsusd's config is the founded hub's rules",
  );
  // An owner book peers with nobody, and its config says nothing.
  assertEquals(
    invalidHubs({ ...base, configs: { [DSUSD_BOOK]: "{}" } }),
    undefined,
  );
});
