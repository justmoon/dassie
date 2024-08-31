export type Ratio = [numerator: bigint, denominator: bigint]

export function multiplyByRatio(
  value: bigint,
  [numerator, denominator]: Ratio,
) {
  return (value * numerator) / denominator
}
