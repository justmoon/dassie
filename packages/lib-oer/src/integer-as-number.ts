import { OerType } from "./base-type"
import { ParseFailure, SerializeFailure } from "./utils/failures"
import type { ParseContext, SerializeContext } from "./utils/parse"
import { type FixedRange, type Range, parseRange } from "./utils/range"

export interface IntegerAsNumberOptions {
  range: FixedRange<number>
  type: "Uint" | "Int"
  size: 8 | 16 | 32
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

abstract class OerIntegerNumber extends OerType<number> {}

export class OerFixedIntegerNumber extends OerIntegerNumber {
  constructor(readonly options: IntegerAsNumberOptions) {
    super()
  }

  clone() {
    return new OerFixedIntegerNumber(this.options)
  }

  parseWithContext({ uint8Array, dataView }: ParseContext, offset: number) {
    const { type, size, range } = this.options

    if (offset + size / 8 > dataView.byteLength) {
      return new ParseFailure(
        `unable to read fixed length integer of size ${
          size / 8
        } bytes - end of buffer`,
        uint8Array,
        uint8Array.byteLength,
      )
    }

    const value = dataView[`get${type}${size}`](offset)

    if (value < range[0]) {
      return new ParseFailure(
        `unable to read fixed length integer of size ${
          size / 8
        } bytes - value ${value} is less than minimum value ${range[0]}`,
        uint8Array,
        offset,
      )
    }

    if (value > range[1]) {
      return new ParseFailure(
        `unable to read fixed length integer of size ${
          size / 8
        } bytes - value ${value} is greater than maximum value ${range[1]}`,
        uint8Array,
        offset,
      )
    }

    return [value, size / 8] as const
  }

  serializeWithContext(value: number) {
    const { type, size, range } = this.options

    const serializer = (context: SerializeContext, offset: number) => {
      if (value < range[0]) {
        return new SerializeFailure(`integer must be >= ${range[0]}`)
      }

      if (value > range[1]) {
        return new SerializeFailure(`integer must be <= ${range[1]}`)
      }

      context.dataView[`set${type}${size}`](offset, value)

      return
    }
    serializer.size = size / 8
    return serializer
  }
}

export const integerAsNumber = (range: Range<number>) => {
  const [minimumValue, maximumValue] = parseRange(range)

  if (minimumValue == undefined || maximumValue == undefined) {
    throw new Error(
      "When using JavaScript numbers, a minimum and maximum must be provided. If your range is unbounded, use integerAsBigint instead.",
    )
  }

  const fixedOptions = {
    range: [minimumValue, maximumValue],
  } as const

  // Fixed size integer encodings
  if (minimumValue >= UINT_MIN_NUMBER) {
    if (maximumValue <= UINT8_MAX_NUMBER) {
      return new OerFixedIntegerNumber({
        ...fixedOptions,
        type: "Uint",
        size: 8,
      })
    } else if (maximumValue <= UINT16_MAX_NUMBER) {
      return new OerFixedIntegerNumber({
        ...fixedOptions,
        type: "Uint",
        size: 16,
      })
    } else if (maximumValue <= UINT32_MAX_NUMBER) {
      return new OerFixedIntegerNumber({
        ...fixedOptions,
        type: "Uint",
        size: 32,
      })
    }
  } else {
    if (minimumValue <= INT8_MIN_NUMBER && maximumValue <= INT8_MAX_NUMBER) {
      return new OerFixedIntegerNumber({
        ...fixedOptions,
        type: "Int",
        size: 8,
      })
    } else if (
      minimumValue <= INT16_MIN_NUMBER &&
      maximumValue <= INT16_MAX_NUMBER
    ) {
      return new OerFixedIntegerNumber({
        ...fixedOptions,
        type: "Int",
        size: 16,
      })
    } else if (
      minimumValue <= INT32_MIN_NUMBER &&
      maximumValue <= INT32_MAX_NUMBER
    ) {
      return new OerFixedIntegerNumber({
        ...fixedOptions,
        type: "Int",
        size: 32,
      })
    }
  }

  throw new TypeError(
    "integerAsNumber only supports up to 32 bits due to JavaScript's limitations, please use integerAsBigint instead",
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
