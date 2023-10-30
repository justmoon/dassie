export type Range<T extends number | bigint> =
  | T
  | undefined
  | NormalizedRange<T>

export type NormalizedRange<T extends number | bigint> = readonly [
  minimum: T | undefined,
  maximum: T | undefined,
]

export type FixedRange<T extends number | bigint> = readonly [
  minimum: T,
  maximum: T,
]

export const parseRange = <T extends number | bigint>(
  range: Range<T>,
): NormalizedRange<T> => {
  if (typeof range === "number" || typeof range === "bigint") {
    return [range, range]
  }

  if (range == undefined) {
    return [undefined, undefined]
  }

  if (range[0] != undefined && range[1] != undefined && range[0] > range[1]) {
    throw new Error(`minimum value must be less than or equal to maximum value`)
  }

  return [range[0], range[1]]
}
