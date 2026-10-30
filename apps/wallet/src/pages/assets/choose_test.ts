import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import { AssetInfoSchema, Realm } from "@dassie/rpc/proto";
import { choice, choosable, initialChoice, offered, toggle } from "./choose.ts";

const asset = (
  name: string,
  fields: { needs?: string[]; active?: boolean; realm?: Realm } = {},
) =>
  create(AssetInfoSchema, {
    name,
    assetCode: name.toUpperCase(),
    assetScale: 6,
    realm: fields.realm ?? Realm.TEST,
    needs: fields.needs ?? [],
    active: fields.active ?? false,
  });

const xrp = asset("xrp");
const dsusd = asset("dsusd", { needs: ["xrp"] });

test("the checklist offers what the node does not hold yet", () => {
  assertEquals(
    offered([asset("xrp", { active: true }), dsusd]).map((a) => a.name),
    ["dsusd"],
  );
  assertEquals(choosable(xrp), true);
  assertEquals(choosable(asset("xrp", { realm: Realm.LIVE })), false);
});

test("XRP starts ticked where it may be chosen", () => {
  assertEquals(initialChoice([xrp, dsusd]), ["xrp"]);
  assertEquals(initialChoice([dsusd]), []);
  assertEquals(initialChoice([asset("xrp", { realm: Realm.LIVE })]), []);
  assertEquals(initialChoice([asset("usd")]), []);
});

test("an asset ticks and locks what it needs", () => {
  assertEquals(choice([xrp, dsusd], []), { ticked: [], neededBy: {} });
  // Ticked by the owner, in the rows' order.
  assertEquals(choice([xrp, dsusd], ["dsusd"]), {
    ticked: ["xrp", "dsusd"],
    neededBy: { xrp: ["dsusd"] },
  });
  // Ticked already, it is locked while DSUSD is, and stays once DSUSD
  // goes.
  assertEquals(choice([xrp, dsusd], ["xrp", "dsusd"]).neededBy, {
    xrp: ["dsusd"],
  });
  assertEquals(choice([xrp, dsusd], toggle(["xrp", "dsusd"], "dsusd")), {
    ticked: ["xrp"],
    neededBy: {},
  });
  // Ticked only for DSUSD, it goes with it.
  assertEquals(choice([xrp, dsusd], toggle(["dsusd"], "dsusd")).ticked, []);
});

test("a need the node holds already is no row", () => {
  assertEquals(choice([dsusd], ["dsusd"]), {
    ticked: ["dsusd"],
    neededBy: {},
  });
  // Unknown names are dropped.
  assertEquals(choice([xrp], ["rlusd"]).ticked, []);
});

test("needs of needs are ticked too", () => {
  const a = asset("a", { needs: ["b"] });
  const b = asset("b", { needs: ["c"] });
  const c = asset("c");
  assertEquals(choice([a, b, c], ["a"]), {
    ticked: ["a", "b", "c"],
    neededBy: { b: ["a"], c: ["b"] },
  });
});
