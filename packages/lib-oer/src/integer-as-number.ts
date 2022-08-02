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

export abstract class OerFixedIntegerNumber extends OerType<number> {
  abstract readonly type: "Uint" | "Int"
  abstract readonly size: 8 | 16 | 32

  constructor(readonly options: IntegerAsNumberOptions) {
    super()
  }

  parseWithContext({ uint8Array, dataView }: ParseContext, offset: number) {
    if (offset + this.size / 8 > dataView.byteLength) {
      return new ParseError(
        `unable to read fixed length integer of size ${
          this.size / 8
        } bytes - end of buffer`,
        uint8Array,
        uint8Array.byteLength
      )
    }

    const value = dataView[`get${this.type}${this.size}`](offset)

    if (value < this.options.range[0]) {
      return new ParseError(
        `unable to read fixed length integer of size ${
          this.size / 8
        } bytes - value ${value} is less than minimum value ${
          this.options.range[0]
        }`,
        uint8Array,
        offset
      )
    }

    if (value > this.options.range[1]) {
      return new ParseError(
        `unable to read fixed length integer of size ${
          this.size / 8
        } bytes - value ${value} is greater than maximum value ${
          this.options.range[1]
        }`,
        uint8Array,
        offset
      )
    }

    return [value, this.size / 8] as const
  }

  serializeWithContext(value: number) {
    const serializer = (context: SerializeContext, offset: number) => {
      if (value < this.options.range[0]) {
        return new SerializeError(`integer must be >= ${this.options.range[0]}`)
      }

      if (value > this.options.range[1]) {
        return new SerializeError(`integer must be <= ${this.options.range[1]}`)
      }

      context.dataView[`set${this.type}${this.size}`](offset, value)

      return
    }
    serializer.size = this.size / 8
    return serializer
  }
}

export class OerUint8Number extends OerFixedIntegerNumber {
  readonly size = 8
  readonly type = "Uint"
}
export class OerUint16Number extends OerFixedIntegerNumber {
  readonly size = 16
  readonly type = "Uint"
}
export class OerUint32Number extends OerFixedIntegerNumber {
  readonly size = 32
  readonly type = "Uint"
}

export class OerInt8Number extends OerFixedIntegerNumber {
  readonly size = 8
  readonly type = "Int"
}
export class OerInt16Number extends OerFixedIntegerNumber {
  readonly size = 16
  readonly type = "Int"
}
export class OerInt32Number extends OerFixedIntegerNumber {
  readonly size = 32
  readonly type = "Int"
}

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
