/**
 * A ledger's deposit address: its QR code, the address to copy, and what
 * the ledger holds back (spec/areas/wallet.md#pages).
 */

import type { JSX } from "@solidjs/web";
import type { Ledger } from "@dassie/rpc/proto";
import { Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { Qr } from "@dassie/ui/qr";
import { CopyButton } from "@dassie/ui/ui";

export function DepositAddress(props: {
  ledger: Ledger;
  address: string;
  /** The test IDs' prefix, before the ledger's ID. */
  testid: string;
  pixels?: number;
  /** Whether to say what the ledger holds back. */
  reserve?: boolean;
}): JSX.Element {
  return (
    <div class="deposit">
      <Qr
        text={props.address}
        label={`Deposit address ${props.address}`}
        pixels={props.pixels}
        testid={`${props.testid}-qr-${props.ledger.id}`}
      />
      <div>
        <p>Deposit address</p>
        <p>
          <code data-testid={`${props.testid}-address-${props.ledger.id}`}>
            {props.address}
          </code>{" "}
          <CopyButton text={props.address} label="Copy address" />
        </p>
        <Show when={props.reserve && props.ledger.reserve > 0n}>
          <p class="muted">
            The ledger holds back{" "}
            <Amount
              value={props.ledger.reserve}
              currency={ledgerCurrency(props.ledger)}
            />{" "}
            as a reserve: never spent, and not in the balance.
          </p>
        </Show>
      </div>
    </div>
  );
}
