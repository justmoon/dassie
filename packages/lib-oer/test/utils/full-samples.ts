import {
  OerType,
  bitstring,
  boolean,
  hexToUint8Array,
  octetString,
  sequence,
  sequenceOf,
  uint8Number,
  uint16Number,
  uint64Bigint,
} from "../../src"

export type FullSample<T> = readonly [
  label: string,
  baseType: OerType<T>,
  value: T,
  hex: string,
]

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export const constantTests: FullSample<any>[] = [
  ["empty fixed size octet string", octetString(0), hexToUint8Array(""), ""],
  [
    "fixed size octet string",
    octetString(4),
    hexToUint8Array("12345678"),
    "12345678",
  ],
  [
    "variable length octet string",
    octetString(),
    hexToUint8Array("12345678"),
    "0412345678",
  ],
  ["uint8", uint8Number(), 15, "0f"],
  [
    "uint64Bigint",
    uint64Bigint(),
    BigInt("0xffffffffffffffff"),
    "ffffffffffffffff",
  ],
  ["boolean", boolean(), true, "ff"],
  ["bitstring", bitstring(3), [true, false, true], "a0"],
  [
    "sequence",
    sequence({
      a: uint8Number(),
      b: boolean(),
      c: octetString(1),
    }),
    {
      a: 15,
      b: false,
      c: hexToUint8Array("de"),
    },
    "0f00de",
  ],
  [
    "sequenceOf",
    sequenceOf(uint16Number()),
    [0xf1_00, 0xf2_00, 0xf3_00, 0xf4_00],
    "0104f100f200f300f400",
  ],
]
