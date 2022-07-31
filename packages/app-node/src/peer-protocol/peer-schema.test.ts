import { describe, test } from "vitest"

import { hexToUint8Array } from "@xen-ilp/lib-oer"

import { peerMessage } from "./peer-schema"

describe("xen message codec", () => {
  test("should parse a hello message", ({ expect }) => {
    const uint8Array = hexToUint8Array(
      `80 4a 02 6e 31 00 00 01 82 4b a9 32 a2 19 68 74 74 70 73 3a 2f 2f 6e 31 
       2e 6c 6f 63 61 6c 68 6f 73 74 3a 34 30 30 30 01 01 02 6e 32 00 00 00 00 
       00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 
       00 00 00 00 40 6b f2 24 e8 36 b3 71 02 77 31 f9 f5 cc 05 83 cc 88 d7 58 
       f8 80 ea e3 5a bc c4 a9 08 b6 b0 5f a0 1c 68 b6 1a a7 ec 86 7c 14 97 ef 
       76 84 f4 9e 1e 27 05 f7 06 e5 ae 68 82 c6 48 df 41 3e aa 50 07`
    )

    const message = peerMessage.parse(uint8Array)

    expect(message).toMatchSnapshot()
  })
})
