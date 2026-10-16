import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import { TransferSchema, TransferState } from "@dassie/rpc/proto";
import {
  accountHref,
  accountRows,
  mergeTransfers,
  netBalance,
  splitPath,
} from "./ledger.ts";

const transfer = (
  id: bigint,
  debit: number,
  credit: number,
  amount: bigint,
  state: TransferState = TransferState.POSTED,
) =>
  create(TransferSchema, {
    id,
    debit,
    credit,
    amount,
    state,
    debitPath: `a${debit}`,
    creditPath: `a${credit}`,
  });

test("paths and their links", () => {
  assertEquals(splitPath("stub+usd:equity/owner/btp/3"), {
    ledger: "stub+usd",
    account: "equity/owner/btp/3",
  });
  assertEquals(splitPath("nonsense"), undefined);
  assertEquals(
    accountHref("stub+usd:equity/owner"),
    "/debug/ledger/stub%2Busd/account/equity/owner",
  );
});

test("each transfer shows the posted balance after it", () => {
  // Account 1 got 100, paid 30, has 5 pending out and 7 voided.
  const rows = accountRows(
    [
      transfer(1n, 0, 1, 100n),
      transfer(2n, 1, 2, 30n),
      transfer(3n, 1, 2, 5n, TransferState.PENDING),
      transfer(4n, 1, 2, 7n, TransferState.VOIDED),
    ],
    1,
    70n,
  );
  assertEquals(rows.map((row) => [row.credit, row.counter, row.balance]), [
    [true, "a0", 100n],
    [false, "a2", 70n],
    [false, "a2", 70n],
    [false, "a2", 70n],
  ]);
  assertEquals(
    netBalance({ creditsPosted: 100n, debitsPosted: 30n, debitsPending: 5n }),
    65n,
  );
});

test("a settlement's row replaces itself when it is resolved", () => {
  const pending = transfer(5n, 1, 2, 9n, TransferState.PENDING);
  const posted = transfer(5n, 1, 2, 9n, TransferState.POSTED);
  const merged = mergeTransfers(
    [transfer(4n, 0, 1, 1n), pending],
    [posted],
    10,
  );
  assertEquals(merged.map((t) => [t.id, t.state]), [
    [4n, TransferState.POSTED],
    [5n, TransferState.POSTED],
  ]);
  assertEquals(mergeTransfers(merged, [transfer(6n, 0, 1, 1n)], 2).length, 2);
});
