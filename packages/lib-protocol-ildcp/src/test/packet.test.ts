import { base64ToUint8Array } from "uint8array-extras"
import { describe, test } from "vitest"

import { parseIldcpResponse, serializeIldcpResponse } from "../schema"
import packetsFixtures from "./fixtures/packets"

describe("Packet Fixtures", () => {
  for (const [name, fixture] of Object.entries(packetsFixtures)) {
    const wantBuffer = base64ToUint8Array(fixture.buffer)
    const wantPacket = fixture.packet

    test("deserializes " + name, ({ expect }) => {
      const gotPacket = parseIldcpResponse(wantBuffer)
      expect(gotPacket).toEqual(wantPacket)
    })

    if ("decodeOnly" in fixture && fixture.decodeOnly) continue

    test("serializes " + name, ({ expect }) => {
      const gotBuffer = serializeIldcpResponse(wantPacket)
      expect(gotBuffer).toEqual(wantBuffer)
    })
  }
})
