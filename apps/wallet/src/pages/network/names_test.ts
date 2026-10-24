import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import { NameAtRegistrarSchema, NameState } from "@dassie/rpc/proto";
import {
  atRegistrar,
  nameHint,
  nameState,
  registrable,
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
  assertEquals(nameHint(NameState.HELD), undefined);
  assertEquals(nameHint(NameState.ABANDONED), "You can register it again.");
});

test("only names the node holds are renewed, and only finished ones registered again", () => {
  assertEquals(renewable(NameState.HELD), true);
  assertEquals(renewable(NameState.LAPSING), true);
  assertEquals(renewable(NameState.WAITING), false);
  assertEquals(registrable(NameState.ABANDONED), true);
  assertEquals(registrable(NameState.NOT_HELD), true);
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
