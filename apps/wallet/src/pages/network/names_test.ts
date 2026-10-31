import { test } from "node:test";
import { assert, assertEquals, assertStringIncludes } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import { NameAtRegistrarSchema, NameState } from "@dassie/rpc/proto";
import {
  atRegistrar,
  nameHint,
  nameState,
  registrable,
  registrarsWorthSeeing,
  renewable,
} from "./names.ts";

const NOW_MS = 1_800_000_000_000;
const NODE = "dOwn";

test("a name's state reads as the node reports it", () => {
  assertEquals(nameState(NameState.HELD), { text: "yours", tone: "good" });
  assertEquals(nameState(NameState.COMMITTING).tone, "plain");
  assertEquals(nameState(NameState.LOST).tone, "bad");
  // A state this wallet does not know says only that.
  assertEquals(nameState(99 as NameState).text, "unknown");
  assertEquals(nameHint(NameState.HELD, 0n, NOW_MS), undefined);
  assertEquals(
    nameHint(NameState.ABANDONED, 0n, NOW_MS),
    "You can register it again.",
  );
});

test("a name under way reads as registering, whatever its step", () => {
  for (
    const state of [
      NameState.COMMITTING,
      NameState.UNSEALING,
      NameState.WAITING,
    ]
  ) {
    assertEquals(nameState(state), { text: "registering", tone: "plain" });
  }
});

test("a name registering says when it is yours, and that nothing is to do", () => {
  const decided = BigInt(NOW_MS + 52 * 60_000);
  const before = nameHint(NameState.UNSEALING, decided, NOW_MS) ?? "";
  assert(before.startsWith("Yours in 52 min, around "), before);
  assertStringIncludes(before, "unless someone registered it before you");
  assertStringIncludes(before, "Nothing for you to do");
  // Its time passed, the node waits for the registrars' lists.
  assertStringIncludes(
    nameHint(NameState.WAITING, decided, Number(decided)) ?? "",
    "Any moment now",
  );
  // While it commits, it says what the node does, and that nothing is to do.
  assertStringIncludes(
    nameHint(NameState.COMMITTING, decided, NOW_MS) ?? "",
    "Nothing for you to do",
  );
  // A node that does not say when still says nothing is to do.
  const unknown = nameHint(NameState.UNSEALING, 0n, NOW_MS) ?? "";
  assertStringIncludes(unknown, "waiting period ends");
  assertStringIncludes(unknown, "Nothing for you to do");
});

test("only names the node holds are renewed, and only finished ones registered again", () => {
  assertEquals(renewable(NameState.HELD), true);
  assertEquals(renewable(NameState.LAPSING), true);
  assertEquals(renewable(NameState.WAITING), false);
  assertEquals(registrable(NameState.ABANDONED), true);
  assertEquals(registrable(NameState.NOT_HELD), true);
  // The node refuses a split name until the registrars release it.
  assertEquals(registrable(NameState.SPLIT), false);
  assertEquals(registrable(NameState.COMMITTING), false);
  assertEquals(registrable(NameState.HELD), false);
});

test("a registrar's standing says what it logged, took and lists", () => {
  const at = (fields: {
    receipt?: boolean;
    unsealed?: boolean;
    refusedStatus?: number;
    listedHolder?: string;
    listedExpiresAtUnixMs?: bigint;
    listFresh?: boolean;
  }) =>
    create(NameAtRegistrarSchema, {
      registrar: "dR",
      listFresh: true,
      ...fields,
    });
  assertEquals(atRegistrar(at({}), NODE, NOW_MS), "nothing yet");
  assertEquals(
    atRegistrar(at({ receipt: true }), NODE, NOW_MS),
    "logged the commitment",
  );
  assertEquals(
    atRegistrar(at({ receipt: true, unsealed: true }), NODE, NOW_MS),
    "took the unsealing",
  );
  assertEquals(
    atRegistrar(at({ refusedStatus: 503 }), NODE, NOW_MS),
    "full, asking again",
  );
  assertEquals(
    atRegistrar(
      at({
        listedHolder: NODE,
        listedExpiresAtUnixMs: BigInt(NOW_MS + 2 * 86_400_000),
      }),
      NODE,
      NOW_MS,
    ),
    "lists it for you (expiry in 2 d)",
  );
  assertEquals(
    atRegistrar(
      at({ listedHolder: "dOther", listFresh: false }),
      NODE,
      NOW_MS,
    ),
    "lists another node, its list is not current",
  );
});

test("the registrars show unasked only when one refused or lists another node", () => {
  const at = (fields: { refusedStatus?: number; listedHolder?: string }) =>
    create(NameAtRegistrarSchema, { registrar: "dR", ...fields });
  assertEquals(registrarsWorthSeeing([at({}), at({})], NODE), false);
  assertEquals(
    registrarsWorthSeeing([at({ listedHolder: NODE })], NODE),
    false,
  );
  assertEquals(
    registrarsWorthSeeing([at({}), at({ refusedStatus: 409 })], NODE),
    true,
  );
  assertEquals(
    registrarsWorthSeeing([at({ listedHolder: "dOther" })], NODE),
    true,
  );
});
