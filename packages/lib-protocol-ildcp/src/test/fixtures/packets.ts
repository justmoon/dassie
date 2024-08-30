import type { IldcpResponse } from "../../schema"

export default {
  response: {
    packet: {
      address: "test.alice",
      assetScale: 9,
      assetCode: "XRP",
    },
    buffer: "CnRlc3QuYWxpY2UJA1hSUA==",
  },
} satisfies Record<
  string,
  {
    packet: IldcpResponse
    buffer: string
  }
>
