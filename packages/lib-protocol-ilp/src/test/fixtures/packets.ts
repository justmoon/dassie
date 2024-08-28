import { base64ToUint8Array } from "uint8array-extras"

import { IlpErrorCode } from "../../errors"
import { type IlpPacket, IlpType } from "../../schema"

const EXAMPLE_DATE = "20171223012140549"

export default {
  prepare: {
    packet: {
      type: IlpType.Prepare,
      data: {
        amount: 107n,
        expiresAt: EXAMPLE_DATE,
        executionCondition: base64ToUint8Array(
          "dOETbcccnl8oO+yDRhy/EmHEAU9y1I+N1lRToLhOfeE=",
        ),
        destination: "example.alice",
        data: base64ToUint8Array(
          "XbND/cQYmPbfQgIykTncJC3Q9VioEbRrKJGP2rN8bLA=",
        ),
      },
    },
    buffer:
      "DGgAAAAAAAAAazIwMTcxMjIzMDEyMTQwNTQ5dOETbcccnl8oO+yDRhy/EmHEAU9y1I+N1lRToLhOfeENZXhhbXBsZS5hbGljZSBds0P9xBiY9t9CAjKROdwkLdD1WKgRtGsokY/as3xssA==",
  },
  fulfill: {
    packet: {
      type: IlpType.Fulfill,
      data: {
        fulfillment: base64ToUint8Array(
          "w4ZrSHSczxE7LhXCXSQH+/wUR2/nKWuxvxvNnm5BZlA=",
        ),
        data: base64ToUint8Array(
          "Zz/r14ozso4cDbFMmgYlGgX6gx7U7ZHrzRUOcknC5gA=",
        ),
      },
    },
    buffer:
      "DUHDhmtIdJzPETsuFcJdJAf7/BRHb+cpa7G/G82ebkFmUCBnP+vXijOyjhwNsUyaBiUaBfqDHtTtkevNFQ5yScLmAA==",
  },
  reject: {
    packet: {
      type: IlpType.Reject,
      data: {
        code: IlpErrorCode.F01_INVALID_PACKET,
        triggeredBy: "example.us.bob",
        message: "missing destination. ledger=example.us.",
        data: base64ToUint8Array("AAAABBBB"),
      },
    },
    buffer:
      "DkFGMDEOZXhhbXBsZS51cy5ib2InbWlzc2luZyBkZXN0aW5hdGlvbi4gbGVkZ2VyPWV4YW1wbGUudXMuBgAAAAQQQQ==",
  },
} satisfies Record<
  string,
  {
    packet: IlpPacket
    buffer: string
  }
>
