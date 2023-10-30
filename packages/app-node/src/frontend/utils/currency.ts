export const parseDecimalToBigInt = (
  amountAsDecimal: string,
  totalPrecision: number,
): bigint => {
  const [integer, decimal] = `${amountAsDecimal}.`.split(".")
  const integerAmount = BigInt(integer!) * BigInt(10 ** totalPrecision)
  const decimalAmount = decimal
    ? BigInt(decimal) * BigInt(10 ** (totalPrecision - decimal.length))
    : 0n
  return integerAmount + decimalAmount
}
