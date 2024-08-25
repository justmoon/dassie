import type { CurrencySpecification } from "../../types/currency"
import { combine } from "../../utils/class-helper"

export interface AmountProperties extends React.HTMLAttributes<HTMLDivElement> {
  value: bigint
  currency: CurrencySpecification
}

export const Amount = ({
  value,
  currency,
  className,
  ...remainingProperties
}: AmountProperties) => {
  const negative = value < 0n ? "-" : ""
  const absoluteBalance = negative ? -value : value

  const totalPrecisionDividend = 10n ** BigInt(currency.totalPrecision)

  const integerPart = (absoluteBalance / totalPrecisionDividend).toLocaleString(
    undefined,
    { useGrouping: true },
  )
  const fractionalPart = (absoluteBalance % totalPrecisionDividend)
    .toString()
    .padStart(currency.totalPrecision, "0")
  const emphasizedPart = fractionalPart.slice(0, currency.displayPrecision)
  const deemphasizedPart = fractionalPart.slice(currency.displayPrecision)

  return (
    <div
      className={combine("inline-flex items-baseline", className)}
      {...remainingProperties}
    >
      {negative ?
        <div>&minus;&#x2009;</div>
      : null}
      <div>{currency.symbol}&#x2009;</div>
      <div>{integerPart}</div>
      <div>.</div>
      <div>{emphasizedPart}</div>
      <div className="text-muted-foreground">
        {deemphasizedPart.slice(0, 2)}
      </div>
    </div>
  )
}
