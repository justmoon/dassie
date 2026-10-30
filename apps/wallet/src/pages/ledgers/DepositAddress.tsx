/**
 * A ledger's deposit address: its QR code and the address to copy
 * (spec/areas/wallet.md#pages).
 */

import type { JSX } from "@solidjs/web";
import type { Ledger } from "@dassie/rpc/proto";
import { Qr } from "@dassie/ui/qr";
import { CopyButton } from "@dassie/ui/ui";

export function DepositAddress(props: {
  ledger: Ledger;
  address: string;
  /** The test IDs' prefix, before the ledger's ID. */
  testid: string;
  pixels?: number;
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
        <p class="deposit-label">Deposit address</p>
        <p class="deposit-address">
          <code data-testid={`${props.testid}-address-${props.ledger.id}`}>
            {props.address}
          </code>{" "}
          <CopyButton text={props.address} label="Copy address" />
        </p>
      </div>
    </div>
  );
}
