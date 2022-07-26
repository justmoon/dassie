export type Range<T extends number | bigint> =
  | T
  | undefined
  | readonly [minimum: T | undefined, maximum: T | undefined]

export const parseRange = <T extends number | bigint>(
  range: Range<T>
): { minimum: T | undefined; maximum: T | undefined } => {
  if (typeof range === "number" || typeof range === "bigint") {
    return { minimum: range, maximum: range }
  }

  if (range == undefined) {
    return { minimum: undefined, maximum: undefined }
  }

  if (range[0] != undefined && range[1] != undefined && range[0] > range[1]) {
    throw new Error(`minimum value must be less than or equal to maximum value`)
  }

  return { minimum: range[0], maximum: range[1] }
}
