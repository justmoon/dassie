import { OerType } from "./base-type"
import { ParseError } from "./utils/errors"
import type { ParseContext, SerializeContext } from "./utils/parse"
import { Range, parseRange } from "./utils/range"

interface RealAsNumberOptions {
  mantissa?: Range<number>
  base?: Range<number>
  exponent?: Range<number>
}

interface FixedRealAsNumberOptions {
  minimumMantissa: number
  maximumMantissa: number
  minimumBase: number
  maximumBase: number
  minimumExponent: number
  maximumExponent: number
}

export const FLOAT32_MIN_MANTISSA_NUMBER = 1 - 2 ** 24
export const FLOAT32_MAX_MANTISSA_NUMBER = 2 ** 24 - 1
export const FLOAT32_MIN_EXPONENT_NUMBER = -149
export const FLOAT32_MAX_EXPONENT_NUMBER = 104

export const FLOAT64_MIN_MANTISSA_NUMBER = -Number.MAX_SAFE_INTEGER
export const FLOAT64_MAX_MANTISSA_NUMBER = Number.MAX_SAFE_INTEGER
export const FLOAT64_MIN_EXPONENT_NUMBER = -1074
export const FLOAT64_MAX_EXPONENT_NUMBER = 971

export const createOerFixedRealNumber = (size: 32 | 64) => {
  const byteSize = size / 8

  return class extends OerType<number> {
    constructor(readonly options: FixedRealAsNumberOptions) {
      super()
    }

    parseWithContext({ uint8Array, dataView }: ParseContext, offset: number) {
      if (offset + byteSize > dataView.byteLength) {
        return new ParseError(
          `unable to read fixed length real of size ${byteSize} bytes - end of buffer`,
          uint8Array,
          uint8Array.byteLength
        )
      }

      const value = dataView[`getFloat${size}`](offset)

      return [value, byteSize] as const
    }

    serializeWithContext(value: number) {
      return [
        (context: SerializeContext, offset: number) => {
          context.dataView[`setFloat${size}`](offset, value)

          return
        },
        byteSize,
      ] as const
    }
  }
}

const OerFloat32Number = createOerFixedRealNumber(32)
const OerFloat64Number = createOerFixedRealNumber(64)

export const realAsNumber = ({
  mantissa = [FLOAT64_MIN_MANTISSA_NUMBER, FLOAT64_MAX_MANTISSA_NUMBER],
  base = 2,
  exponent = [FLOAT64_MIN_EXPONENT_NUMBER, FLOAT64_MAX_EXPONENT_NUMBER],
}: RealAsNumberOptions = {}) => {
  const [minimumMantissa, maximumMantissa] = parseRange(mantissa)

  if (
    minimumMantissa == undefined ||
    minimumMantissa < FLOAT64_MIN_MANTISSA_NUMBER
  ) {
    throw new Error(
      `minimum mantissa must be >= ${FLOAT64_MIN_MANTISSA_NUMBER}`
    )
  }

  if (
    maximumMantissa == undefined ||
    maximumMantissa > FLOAT64_MAX_MANTISSA_NUMBER
  ) {
    throw new Error(
      `maximum mantissa must be <= ${FLOAT64_MAX_MANTISSA_NUMBER}`
    )
  }

  const [minimumBase, maximumBase] = parseRange(base)

  if (minimumBase !== 2 || maximumBase !== 2) {
    throw new Error(`base must be 2`)
  }

  const [minimumExponent, maximumExponent] = parseRange(exponent)

  if (
    minimumExponent == undefined ||
    minimumExponent < FLOAT64_MIN_EXPONENT_NUMBER
  ) {
    throw new Error(
      `minimum exponent must be >= ${FLOAT64_MIN_EXPONENT_NUMBER}`
    )
  }

  if (
    maximumExponent == undefined ||
    maximumExponent > FLOAT64_MAX_EXPONENT_NUMBER
  ) {
    throw new Error(
      `maximum exponent must be <= ${FLOAT64_MAX_EXPONENT_NUMBER}`
    )
  }

  const fixedOptions = {
    minimumMantissa,
    maximumMantissa,
    minimumBase,
    maximumBase,
    minimumExponent,
    maximumExponent,
  }

  // Fixed size integer encodings
  return minimumMantissa >= FLOAT32_MIN_MANTISSA_NUMBER &&
    maximumMantissa <= FLOAT32_MAX_MANTISSA_NUMBER &&
    minimumExponent >= FLOAT32_MIN_EXPONENT_NUMBER &&
    maximumExponent <= FLOAT32_MAX_EXPONENT_NUMBER
    ? new OerFloat32Number(fixedOptions)
    : new OerFloat64Number(fixedOptions)
}

export const float32Number = () =>
  realAsNumber({
    mantissa: [FLOAT32_MIN_MANTISSA_NUMBER, FLOAT32_MAX_MANTISSA_NUMBER],
    base: 2,
    exponent: [FLOAT32_MIN_EXPONENT_NUMBER, FLOAT32_MAX_EXPONENT_NUMBER],
  })

export const float64Number = () =>
  realAsNumber({
    mantissa: [FLOAT64_MIN_MANTISSA_NUMBER, FLOAT64_MAX_MANTISSA_NUMBER],
    base: 2,
    exponent: [FLOAT64_MIN_EXPONENT_NUMBER, FLOAT64_MAX_EXPONENT_NUMBER],
  })
