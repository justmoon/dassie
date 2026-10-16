import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { devLoginProof } from "./dev-login.ts";

const PROOF = "ab".repeat(32);

test("a dev login link carries 32 bytes of hex", () => {
  assertEquals(devLoginProof(`#dassie-dev-login=${PROOF}`), PROOF);
  assertEquals(
    devLoginProof(`#dassie-dev-login=${PROOF.toUpperCase()}`),
    PROOF,
  );
  assertEquals(devLoginProof(""), undefined);
  assertEquals(devLoginProof("#other"), undefined);
  assertEquals(devLoginProof(`#dassie-dev-login=${PROOF}00`), undefined);
  assertEquals(devLoginProof("#dassie-dev-login=xyz"), undefined);
});
