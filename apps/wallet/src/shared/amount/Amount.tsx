/** An amount in its currency, with the digits beyond cents muted. */

import type { JSX } from "@solidjs/web";
import { type Currency, currencyOf, formatAmount } from "./currency.ts";

export function Amount(props: {
  value: bigint;
  currency: Currency;
  size?: "md" | "lg";
  testid?: string;
}): JSX.Element {
  const parts = () => formatAmount(props.value, props.currency);
  return (
    <span
      class={`amount amount-${props.size ?? "md"}`}
      data-testid={props.testid}
      data-value={props.value.toString()}
      aria-label={parts().text}
    >
      {parts().negative ? "-" : ""}
      <span class="amount-symbol">{parts().symbol}</span> {parts().integer}
      {parts().fraction ? `.${parts().fraction}` : ""}
      <span class="amount-extra" aria-hidden="true">{parts().extra}</span>
    </span>
  );
}

/** The currency of a ledger, as the node describes it. */
export function ledgerCurrency(ledger: {
  assetCode: string;
  assetScale: number;
}): Currency {
  return currencyOf(ledger.assetCode, ledger.assetScale);
}
