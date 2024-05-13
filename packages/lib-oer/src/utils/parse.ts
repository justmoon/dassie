import type { Tagged } from "type-fest"

export interface BaseContext {
  readonly uint8Array: Uint8Array
  readonly dataView: DataView
}

export interface ParseContext extends BaseContext {
  readonly allowNoncanonical: boolean
}

export type SerializeContext = BaseContext

export type SafeUnsignedInteger = Tagged<number, "Integer in range [0, 2^53)">

export const isSafeUnsignedInteger = (
  value: number,
): value is SafeUnsignedInteger => Number.isSafeInteger(value) && value >= 0

export const byteLength = (sample: SafeUnsignedInteger) =>
  sample <= 0xff ? 1
  : sample <= 0xff_ff ? 2
  : sample <= 0xff_ff_ff ? 3
  : sample <= 0xff_ff_ff_ff ? 4
  : sample <= 0xff_ff_ff_ff_ff ? 5
  : sample <= 0xff_ff_ff_ff_ff_ff ? 6
  : 7
