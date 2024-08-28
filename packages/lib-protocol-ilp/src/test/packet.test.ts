import { base64ToUint8Array } from "uint8array-extras"
import { describe, test } from "vitest"

import { parseIlpPacket, serializeIlpPacket } from "../"
import packetsFixtures from "./fixtures/packets"

describe("Packet Fixtures", () => {
  for (const [name, fixture] of Object.entries(packetsFixtures)) {
    const wantBuffer = base64ToUint8Array(fixture.buffer)
    const wantPacket = fixture.packet

    test("deserializes " + name, ({ expect }) => {
      const gotPacket = parseIlpPacket(wantBuffer)
      expect(gotPacket).toEqual(wantPacket)
    })

    if ("decodeOnly" in fixture && fixture.decodeOnly) continue

    test("serializes " + name, ({ expect }) => {
      const gotBuffer = serializeIlpPacket(wantPacket)
      expect(gotBuffer).toEqual(wantBuffer)
    })
  }
})
