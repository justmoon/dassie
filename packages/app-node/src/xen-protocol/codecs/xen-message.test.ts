import { describe, test } from "vitest"

import { hexToUint8Array } from "@xen-ilp/lib-oer"

import { parseEnvelope } from "./xen-message"

describe("xen message codec", () => {
  test("should parse a hello message", ({ expect }) => {
    const uint8Array = hexToUint8Array(
      `02 6e 33 6b 80 00 00 01 82 41 d4 8c 56 19 68 74 74 70 73 3a 2f 2f 6e 33 
       2e 6c 6f 63 61 6c 68 6f 73 74 3a 34 30 30 32 01 02 02 6e 31 00 00 00 00 
       00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 
       00 00 00 00 02 6e 32 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 
       00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 39 d2 f4 1b 0f 8a 09 77 68 
       0a a8 06 3f d0 78 74 1c f2 df b3 0c 95 28 2c 07 c0 d9 0f 83 eb d9 74 78 
       f7 29 b1 46 e7 1b c9 21 dd ed 17 c6 52 a6 fa 34 ad 29 1d ea a7 05 3c 6d 
       a6 c2 39 e5 f9 e1 05`
    )

    const message = parseEnvelope(uint8Array)

    expect(message).toMatchSnapshot()
  })
})
