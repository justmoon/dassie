import { getLengthPrefixAsHex } from "./sample-length-prefix"

const positiveSampleNumbers = [
  1n,
  2n,
  0x7fn,
  0x80n,
  0xfdn,
  0xfen,
  0xffn,
  0x1_00n,
  0x12_34n,
  0x7f_ffn,
  0x80_00n,
  0xff_fen,
  0xff_ffn,
  0x1_00_00n,
  0x12_34_56n,
  0x7f_ff_ffn,
  0x80_00_00n,
  0xff_ff_fen,
  0xff_ff_ffn,
  0x1_00_00_00n,
  0x01_23_45_67n,
  0x7f_ff_ff_ffn,
  0x80_00_00_00n,
  0xff_ff_ff_fdn,
  0xff_ff_ff_fen,
  0xff_ff_ff_ffn,
  0x1_00_00_00_00n,
  0x1f_ff_ff_ff_ff_ff_ffn, // Number.MAX_SAFE_INTEGER
  0x01_23_45_67_89_ab_cd_efn,
  0x7f_ff_ff_ff_ff_ff_ff_ffn,
  0x80_00_00_00_00_00_00_00n,
  0xff_ff_ff_ff_ff_ff_ff_fdn,
  0xff_ff_ff_ff_ff_ff_ff_fen,
  0xff_ff_ff_ff_ff_ff_ff_ffn,
  0x1_00_00_00_00_00_00_00_00n,
  0xff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ffn,
  0x1_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00n,
  0xff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ff_ffn,
  0x1_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00n,
  2n ** 1024n - 2n,
  2n ** 1024n - 1n,
  2n ** 1024n,
  2n ** 1024n + 1n,
  2n ** 1024n + 2n,
  2n ** 2048n - 2n,
  2n ** 2048n - 1n,
  2n ** 2048n,
  2n ** 2048n + 1n,
  2n ** 2048n + 2n,
]

const negativeSampleNumbers = positiveSampleNumbers
  .map((value) => -value)
  .reverse()

export const sampleNumbers = [
  ...negativeSampleNumbers,
  0n,
  ...positiveSampleNumbers,
]

const fitsLength = (
  value: bigint,
  signed: boolean,
  bitLengthBigInt: bigint,
) => {
  if (signed) {
    return value < 0n ?
        value >= -(2n ** (bitLengthBigInt - 1n))
      : value < 2n ** (bitLengthBigInt - 1n)
  } else {
    return value < 0n ? false : value < 2n ** bitLengthBigInt
  }
}

const getValueAsHex = (value: bigint, byteLength: number) => {
  const bitLengthBigInt = BigInt(byteLength) * 8n
  return value < 0n ?
      // Calculate two's complement
      (2n ** bitLengthBigInt + value).toString(16)
    : value.toString(16).padStart(byteLength * 2, "0")
}

export const getFixedLengthSamples = (signed: boolean, byteLength: number) => {
  const bitLengthBigInt = BigInt(byteLength) * 8n

  return (
    sampleNumbers
      // Get all numbers that are in range for the given length
      .filter((value) => fitsLength(value, signed, bitLengthBigInt))
      // Get the hex representation of the number in the given length
      .map((value) => {
        return [value, getValueAsHex(value, byteLength)] as const
      })
  )
}

export const getVariableLengthSamples = (signed: boolean) => {
  return sampleNumbers
    .filter((value) => signed || value >= 0n)
    .map((value) => {
      for (let bitLengthBigInt = 8n; ; bitLengthBigInt += 8n) {
        if (fitsLength(value, signed, bitLengthBigInt)) {
          const byteLength = Number(bitLengthBigInt / 8n)
          return [
            value,
            getLengthPrefixAsHex(byteLength) + getValueAsHex(value, byteLength),
          ] as const
        }
      }
    })
}

export const getLengthPrefixSamples = () => {
  // We currently support four byte lengths maximum in length prefixes
  const bitLengthBigInt = 4n * 8n

  return (
    sampleNumbers
      // Get all numbers that are in range for the given length
      .filter((value) => fitsLength(value, false, bitLengthBigInt))
      // Get the hex representation of the number in the given length
      .map((value) => {
        return [Number(value), getLengthPrefixAsHex(Number(value))] as const
      })
  )
}
