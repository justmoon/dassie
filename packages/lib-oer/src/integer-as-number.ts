import { OerType } from "./base-type"
import { ParseError, SerializeError } from "./utils/errors"
import type { ParseContext, SerializeContext } from "./utils/parse"
import { FixedRange, Range, parseRange } from "./utils/range"

export interface IntegerAsNumberOptions {
  range: FixedRange<number>
}

export const UINT_MIN_NUMBER = 0
export const UINT8_MAX_NUMBER = 255
export const UINT16_MAX_NUMBER = 65_535
export const UINT32_MAX_NUMBER = 4_294_967_295

export const INT8_MIN_NUMBER = -128
export const INT8_MAX_NUMBER = 127
export const INT16_MIN_NUMBER = -32_768
export const INT16_MAX_NUMBER = 32_767
export const INT32_MIN_NUMBER = -2_147_483_648
export const INT32_MAX_NUMBER = 2_147_483_647

export const createOerFixedIntegerNumber = (
  size: 8 | 16 | 32,
  type: "Uint" | "Int"
) => {
  const byteSize = size / 8

  return class extends OerType<number> {
    constructor(readonly options: IntegerAsNumberOptions) {
      super()
    }

    parseWithContext({ uint8Array, dataView }: ParseContext, offset: number) {
      if (offset + byteSize > dataView.byteLength) {
        return new ParseError(
          `unable to read fixed length integer of size ${byteSize} bytes - end of buffer`,
          uint8Array,
          uint8Array.byteLength
        )
      }

      const value = dataView[`get${type}${size}`](offset)

      if (value < this.options.range[0]) {
        return new ParseError(
          `unable to read fixed length integer of size ${byteSize} bytes - value ${value} is less than minimum value ${this.options.range[0]}`,
          uint8Array,
          offset
        )
      }

      if (value > this.options.range[1]) {
        return new ParseError(
          `unable to read fixed length integer of size ${byteSize} bytes - value ${value} is greater than maximum value ${this.options.range[1]}`,
          uint8Array,
          offset
        )
      }

      return [value, byteSize] as const
    }

    serializeWithContext(value: number) {
      return [
        (context: SerializeContext, offset: number) => {
          if (value < this.options.range[0]) {
            return new SerializeError(
              `integer must be >= ${this.options.range[0]}`
            )
          }

          if (value > this.options.range[1]) {
            return new SerializeError(
              `integer must be <= ${this.options.range[1]}`
            )
          }

          context.dataView[`set${type}${size}`](offset, value)

          return
        },
        byteSize,
      ] as const
    }
  }
}

export const OerUint8Number = createOerFixedIntegerNumber(8, "Uint")
export const OerUint16Number = createOerFixedIntegerNumber(16, "Uint")
export const OerUint32Number = createOerFixedIntegerNumber(32, "Uint")

export const OerInt8Number = createOerFixedIntegerNumber(8, "Int")
export const OerInt16Number = createOerFixedIntegerNumber(16, "Int")
export const OerInt32Number = createOerFixedIntegerNumber(32, "Int")

export const integerAsNumber = (range: Range<number>) => {
  const [minimumValue, maximumValue] = parseRange(range)

  if (minimumValue == undefined || maximumValue == undefined) {
    throw new Error(
      "When using JavaScript numbers, a minimum and maximum must be provided. If your range is actually unbounded, use integerAsBigint instead."
    )
  }

  const fixedOptions: IntegerAsNumberOptions = {
    range: [minimumValue, maximumValue],
  }

  // Fixed size integer encodings
  if (minimumValue >= UINT_MIN_NUMBER) {
    if (maximumValue <= UINT8_MAX_NUMBER) {
      return new OerUint8Number(fixedOptions)
    } else if (maximumValue <= UINT16_MAX_NUMBER) {
      return new OerUint16Number(fixedOptions)
    } else if (maximumValue <= UINT32_MAX_NUMBER) {
      return new OerUint32Number(fixedOptions)
    }
  } else {
    if (minimumValue <= INT8_MIN_NUMBER && maximumValue <= INT8_MAX_NUMBER) {
      return new OerInt8Number(fixedOptions)
    } else if (
      minimumValue <= INT16_MIN_NUMBER &&
      maximumValue <= INT16_MAX_NUMBER
    ) {
      return new OerInt16Number(fixedOptions)
    } else if (
      minimumValue <= INT32_MIN_NUMBER &&
      maximumValue <= INT32_MAX_NUMBER
    ) {
      return new OerInt32Number(fixedOptions)
    }
  }

  throw new TypeError(
    "integerAsNumber only supports up to 32 bits due to JavaScript's limitations, please use integerAsBigint instead"
  )
}

export const uint8Number = () =>
  integerAsNumber([UINT_MIN_NUMBER, UINT8_MAX_NUMBER])
export const uint16Number = () =>
  integerAsNumber([UINT_MIN_NUMBER, UINT16_MAX_NUMBER])
export const uint32Number = () =>
  integerAsNumber([UINT_MIN_NUMBER, UINT32_MAX_NUMBER])
export const int8Number = () =>
  integerAsNumber([INT8_MIN_NUMBER, INT8_MAX_NUMBER])
export const int16Number = () =>
  integerAsNumber([INT16_MIN_NUMBER, INT16_MAX_NUMBER])
export const int32Number = () =>
  integerAsNumber([INT32_MIN_NUMBER, INT32_MAX_NUMBER])
