import { base64ToUint8Array } from "uint8array-extras"

import { FrameType, type StreamPacket } from "../../packets/schema"

export default {
  "sequence:0": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [],
    },
    buffer: "AQwBAAEAAQA=",
  },
  "sequence:max_js": {
    packet: {
      version: 1,
      sequence: 9_007_199_254_740_991n,
      packetType: 12,
      amount: 0n,
      frames: [],
    },
    buffer: "AQwHH////////wEAAQA=",
  },
  "sequence:max_uint_64": {
    packet: {
      version: 1,
      sequence: 18_446_744_073_709_551_615n,
      packetType: 12,
      amount: 0n,
      frames: [],
    },
    buffer: "AQwI//////////8BAAEA",
  },
  "type:prepare": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [],
    },
    buffer: "AQwBAAEAAQA=",
  },
  "type:fulfill": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 13,
      amount: 0n,
      frames: [],
    },
    buffer: "AQ0BAAEAAQA=",
  },
  "type:reject": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 14,
      amount: 0n,
      frames: [],
    },
    buffer: "AQ4BAAEAAQA=",
  },
  "amount:0": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [],
    },
    buffer: "AQwBAAEAAQA=",
  },
  "amount:max_js": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 9_007_199_254_740_991n,
      frames: [],
    },
    buffer: "AQwBAAcf////////AQA=",
  },
  "amount:max_uint_64": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 18_446_744_073_709_551_615n,
      frames: [],
    },
    buffer: "AQwBAAj//////////wEA",
  },
  "frame:connection_close": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.ConnectionClose,
          data: {
            errorCode: 1,
            errorMessage: "fail",
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEBBgEEZmFpbA==",
  },
  "frame:connection_new_address": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.ConnectionNewAddress,
          data: {
            sourceAccount: "example.alice",
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQECDg1leGFtcGxlLmFsaWNl",
  },
  "frame:connection_asset_details": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.ConnectionAssetDetails,
          data: {
            sourceAssetCode: "ABC",
            sourceAssetScale: 255,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEHBQNBQkP/",
  },
  "frame:connection_max_data:0": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.ConnectionMaxData,
          data: {
            maxOffset: 0n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEDAgEA",
  },
  "frame:connection_max_data:max_js": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.ConnectionMaxData,
          data: {
            maxOffset: 9_007_199_254_740_991n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEDCAcf////////",
  },
  "frame:connection_max_data:max_uint_64": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.ConnectionMaxData,
          data: {
            maxOffset: 18_446_744_073_709_551_615n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEDCQj//////////w==",
  },
  "frame:connection_data_blocked:0": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.ConnectionDataBlocked,
          data: {
            maxOffset: 0n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEEAgEA",
  },
  "frame:connection_data_blocked:max_js": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.ConnectionDataBlocked,
          data: {
            maxOffset: 9_007_199_254_740_991n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEECAcf////////",
  },
  "frame:connection_data_blocked:max_uint_64": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.ConnectionDataBlocked,
          data: {
            maxOffset: 18_446_744_073_709_551_615n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEECQj//////////w==",
  },
  "frame:connection_max_stream_id:0": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.ConnectionMaxStreamId,
          data: {
            maxStreamId: 0n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEFAgEA",
  },
  "frame:connection_max_stream_id:max_js": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.ConnectionMaxStreamId,
          data: {
            maxStreamId: 9_007_199_254_740_991n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEFCAcf////////",
  },
  "frame:connection_max_stream_id:max_uint_64": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.ConnectionMaxStreamId,
          data: {
            maxStreamId: 18_446_744_073_709_551_615n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEFCQj//////////w==",
  },
  "frame:connection_stream_id_blocked:0": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.ConnectionStreamIdBlocked,
          data: {
            maxStreamId: 0n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEGAgEA",
  },
  "frame:connection_stream_id_blocked:max_js": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.ConnectionStreamIdBlocked,
          data: {
            maxStreamId: 9_007_199_254_740_991n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEGCAcf////////",
  },
  "frame:connection_stream_id_blocked:max_uint_64": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.ConnectionStreamIdBlocked,
          data: {
            maxStreamId: 18_446_744_073_709_551_615n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEGCQj//////////w==",
  },
  "frame:stream_close": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamClose,
          data: {
            streamId: 123n,
            errorCode: 255,
            errorMessage: "an error message",
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEQFAF7/xBhbiBlcnJvciBtZXNzYWdl",
  },
  "frame:stream_money:0": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMoney,
          data: {
            streamId: 123n,
            shares: 0n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQERBAF7AQA=",
  },
  "frame:stream_money:max_js": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMoney,
          data: {
            streamId: 123n,
            shares: 9_007_199_254_740_991n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQERCgF7Bx////////8=",
  },
  "frame:stream_money:max_uint_64": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMoney,
          data: {
            streamId: 123n,
            shares: 18_446_744_073_709_551_615n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQERCwF7CP//////////",
  },
  "frame:stream_max_money:receive_max:0": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMaxMoney,
          data: {
            streamId: 123n,
            receiveMax: 0n,
            totalReceived: 456n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQESBwF7AQACAcg=",
  },
  "frame:stream_max_money:receive_max:max_js": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMaxMoney,
          data: {
            streamId: 123n,
            receiveMax: 9_007_199_254_740_991n,
            totalReceived: 456n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQESDQF7Bx////////8CAcg=",
  },
  "frame:stream_max_money:receive_max:max_uint_64": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMaxMoney,
          data: {
            streamId: 123n,
            receiveMax: 18_446_744_073_709_551_615n,
            totalReceived: 456n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQESDgF7CP//////////AgHI",
  },
  "frame:stream_max_money:receive_max:greater_than_max_uint64": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMaxMoney,
          data: {
            streamId: 123n,
            receiveMax: 18_446_744_073_709_551_616n,
            totalReceived: 456n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQESDwF7CQEAAAAAAAAAAAIByA==",
  },
  "frame:stream_max_money:total_received:0": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMaxMoney,
          data: {
            streamId: 123n,
            receiveMax: 456n,
            totalReceived: 0n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQESBwF7AgHIAQA=",
  },
  "frame:stream_max_money:total_received:max_js": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMaxMoney,
          data: {
            streamId: 123n,
            receiveMax: 456n,
            totalReceived: 9_007_199_254_740_991n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQESDQF7AgHIBx////////8=",
  },
  "frame:stream_max_money:total_received:max_uint_64": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMaxMoney,
          data: {
            streamId: 123n,
            receiveMax: 456n,
            totalReceived: 18_446_744_073_709_551_615n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQESDgF7AgHICP//////////",
  },
  "frame:stream_money_blocked:send_max:0": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMoneyBlocked,
          data: {
            streamId: 123n,
            sendMax: 0n,
            totalSent: 456n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQETBwF7AQACAcg=",
  },
  "frame:stream_money_blocked:send_max:max_js": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMoneyBlocked,
          data: {
            streamId: 123n,
            sendMax: 9_007_199_254_740_991n,
            totalSent: 456n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQETDQF7Bx////////8CAcg=",
  },
  "frame:stream_money_blocked:send_max:max_uint_64": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMoneyBlocked,
          data: {
            streamId: 123n,
            sendMax: 18_446_744_073_709_551_615n,
            totalSent: 456n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQETDgF7CP//////////AgHI",
  },
  "frame:stream_money_blocked:send_max:greater_than_max_uint64": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMoneyBlocked,
          data: {
            streamId: 123n,
            sendMax: 18_446_744_073_709_551_616n,
            totalSent: 456n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQETDwF7CQEAAAAAAAAAAAIByA==",
  },
  "frame:stream_money_blocked:total_sent:0": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMoneyBlocked,
          data: {
            streamId: 123n,
            sendMax: 456n,
            totalSent: 0n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQETBwF7AgHIAQA=",
  },
  "frame:stream_money_blocked:total_sent:max_js": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMoneyBlocked,
          data: {
            streamId: 123n,
            sendMax: 456n,
            totalSent: 9_007_199_254_740_991n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQETDQF7AgHIBx////////8=",
  },
  "frame:stream_money_blocked:total_sent:max_uint_64": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMoneyBlocked,
          data: {
            streamId: 123n,
            sendMax: 456n,
            totalSent: 18_446_744_073_709_551_615n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQETDgF7AgHICP//////////",
  },
  "frame:stream_data": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamData,
          data: {
            streamId: 123n,
            offset: 456n,
            data: base64ToUint8Array("Zm9vYmFy"),
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEUDAF7AgHIBmZvb2Jhcg==",
  },
  "frame:stream_data:offset:0": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamData,
          data: {
            streamId: 123n,
            offset: 0n,
            data: new Uint8Array(),
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEUBQF7AQAA",
  },
  "frame:stream_data:offset:max_js": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamData,
          data: {
            streamId: 123n,
            offset: 9_007_199_254_740_991n,
            data: new Uint8Array(),
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEUCwF7Bx////////8A",
  },
  "frame:stream_data:offset:max_uint_64": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamData,
          data: {
            streamId: 123n,
            offset: 18_446_744_073_709_551_615n,
            data: new Uint8Array(),
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEUDAF7CP//////////AA==",
  },
  "frame:stream_max_data:offset:0": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMaxData,
          data: {
            streamId: 123n,
            maxOffset: 0n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEVBAF7AQA=",
  },
  "frame:stream_max_data:offset:max_js": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMaxData,
          data: {
            streamId: 123n,
            maxOffset: 9_007_199_254_740_991n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEVCgF7Bx////////8=",
  },
  "frame:stream_max_data:offset:max_uint_64": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamMaxData,
          data: {
            streamId: 123n,
            maxOffset: 18_446_744_073_709_551_615n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEVCwF7CP//////////",
  },
  "frame:stream_data_blocked:offset:0": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamDataBlocked,
          data: {
            streamId: 123n,
            maxOffset: 0n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEWBAF7AQA=",
  },
  "frame:stream_data_blocked:offset:max_js": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamDataBlocked,
          data: {
            streamId: 123n,
            maxOffset: 9_007_199_254_740_991n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEWCgF7Bx////////8=",
  },
  "frame:stream_data_blocked:offset:max_uint_64": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamDataBlocked,
          data: {
            streamId: 123n,
            maxOffset: 18_446_744_073_709_551_615n,
          },
        },
      ],
    },
    buffer: "AQwBAAEAAQEWCwF7CP//////////",
  },
  "frame:stream_receipt": {
    packet: {
      version: 1,
      sequence: 0n,
      packetType: 12,
      amount: 0n,
      frames: [
        {
          type: FrameType.StreamReceipt,
          data: {
            streamId: 1n,
            receipt: base64ToUint8Array(
              "AQAAAAAAAAAAAAAAAAAAAAABAAAAAAAAAfTBIvoCUt67Zy1ZGCP3EOmVFtZzhc85fah8yPnoyL9RMA==",
            ),
          },
        },
      ],
    },
    buffer:
      "AQwBAAEAAQEXPQEBOgEAAAAAAAAAAAAAAAAAAAAAAQAAAAAAAAH0wSL6AlLeu2ctWRgj9xDplRbWc4XPOX2ofMj56Mi/UTA=",
  },
} satisfies Record<
  string,
  {
    packet: StreamPacket
    buffer: string
  }
>
