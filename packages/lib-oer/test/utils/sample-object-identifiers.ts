import { getLengthPrefixAsHex } from "./sample-length-prefix"

// See: https://misc.daniel-marschall.de/asn.1/oid_facts.html
export const objectIdentifiers = [
  ["0.0", "00"],
  ["0.39", "27"],
  ["1.0", "28"],
  ["1.3.6.1.4.1.2021.11.9", "2b060104018f650b09"],
  ["1.3.6.1.4.1.54392.5.1399", "2b0601040183a878058a77"],
  ["1.39", "4f"],
  ["2.0", "50"],
  ["2.47", "7f"],
  ["2.48", "8100"],
  ["2.999", "8837"],
  ["2.999.9007199254740991", "88378fffffffffffff7f"],
  ["2.9007199254740991", "908080808080804f"],
  ["2.9007199254740991.9007199254740991", "908080808080804f8fffffffffffff7f"],
  [
    "2.9223372036854775807.9223372036854775807",
    "8180808080808080804fffffffffffffffff7f",
  ],
] as const

export const getSampleObjectIdentifiers = (): readonly [string, string][] =>
  objectIdentifiers.map(([value, hex]) => [
    value,
    `${getLengthPrefixAsHex(hex.length / 2)}${hex}`,
  ])
