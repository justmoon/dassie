import { describe, test } from "vitest"

import { hexToUint8Array } from "@xen-ilp/lib-oer"

import { parseMessage } from "./xen-message"

describe("xen message codec", () => {
  test("should parse a hello message", ({ expect }) => {
    const uint8Array = hexToUint8Array(
      `00 6d 02 6e 33 00 00 01 82 38 38 06 72 19 68 74 74 70 73 3a 2f 2f 6e 33 
       2e 6c 6f 63 61 6c 68 6f 73 74 3a 34 30 30 32 01 02 02 6e 31 00 00 00 00 
       00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 
       00 00 00 00 02 6e 32 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 
       00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 eb dc 55 15 35 8b 46 1a 1e 
       3a 40 3c f2 d7 25 21 1c ec 0a d2 c6 8a e3 e8 00 f1 85 bd 08 18 76 e7 be 
       db 43 e9 da 2f cf ed 18 52 63 d4 6d f0 53 62 bb 4a a1 df eb 87 97 73 93 
       8b bc 89 56 c0 55 0d`
    )

    const message = parseMessage(uint8Array)

    expect(message).toMatchSnapshot()
  })
})
