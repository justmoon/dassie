/**
 * A dollar token's peg guard on its ledger (spec/areas/wallet.md#pages):
 * why the node paused the scheme, while it is paused, and the token's last
 * price as the guard read it. Nothing for a ledger without a guard.
 */

import type { JSX } from "@solidjs/web";
import type { Ledger } from "@dassie/rpc/proto";
import { Show } from "solid-js";
import { Alert } from "@dassie/ui/ui";
import { tokenScheme } from "../schemes.ts";
import { lastPriceText, pegNotice } from "./peg.ts";

export function PegGuard(props: { ledger: Ledger }): JSX.Element {
  const token = () => tokenScheme(props.ledger.settlementScheme);
  const code = () => token()?.token ?? props.ledger.assetCode;
  const pool = () => token()?.pool ?? `XRP/${code()}`;
  const id = () => props.ledger.id;
  return (
    <Show when={props.ledger.peg}>
      {(peg) => (
        <>
          <Show when={pegNotice(peg(), code(), pool())}>
            {(notice) => (
              <Alert
                title={notice().title}
                tone="danger"
                testid={`ledger-peg-paused-${id()}`}
              >
                {notice().text}
              </Alert>
            )}
          </Show>
          <Show when={lastPriceText(peg())}>
            {(price) => (
              <p class="muted" data-testid={`ledger-peg-price-${id()}`}>
                {code()} on its {pool()} pool: {price()}
              </p>
            )}
          </Show>
        </>
      )}
    </Show>
  );
}
