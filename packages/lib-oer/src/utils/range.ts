export type Range<T extends number | bigint> =
  | T
  | undefined
  | readonly [minimum: T | undefined, maximum: T | undefined]

export const parseNumberRange = (
  range: Range<number>
): { minimum: number | undefined; maximum: number | undefined } => {
  if (typeof range === "number") {
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

export const parseBigintRange = (
  range: Range<bigint>
): { minimum: bigint | undefined; maximum: bigint | undefined } => {
  if (typeof range === "bigint") {
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
