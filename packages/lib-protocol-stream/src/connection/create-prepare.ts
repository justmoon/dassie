import type { IldcpResponse } from "@dassie/lib-protocol-ildcp"

import type { Ratio } from "../math/ratio"
import { FrameType, type StreamFrame } from "../packets/schema"

interface CreatePrepareBuilderParameters {
  fulfillable: boolean
  maxPacketAmount: bigint
}

interface SendMoneyFrameOptions {
  streamId: number
  amount: bigint
  onFulfill?: () => void
  onReject?: () => void
}

interface CloseConnectionFrameOptions {
  errorCode: number
  errorMessage: string
}

interface CloseStreamFrameOptions {
  errorCode: number
  errorMessage: string
}

interface GetPacketOptions {
  exchangeRate: Ratio
}

export function createPrepareBuilder({
  fulfillable,
  maxPacketAmount,
}: CreatePrepareBuilderParameters) {
  const fulfillHandlers = new Array<() => void>()
  const rejectHandlers = new Array<() => void>()
  const streamResponseHandlers = new Array<() => void>()
  let totalSend = 0n

  let newAddress: string | undefined
  let assetDetails: Pick<IldcpResponse, "assetCode" | "assetScale"> | undefined

  const moneyFrames = new Map<number, bigint>()

  let closeConnection: CloseConnectionFrameOptions | undefined
  const closeStreams = new Map<number, CloseStreamFrameOptions>()

  return {
    setNewAddress(address: string) {
      newAddress = address
    },

    setAssetDetails(
      configuration: Pick<IldcpResponse, "assetCode" | "assetScale">,
    ) {
      assetDetails = configuration
    },

    addMoney({ streamId, amount, onFulfill, onReject }: SendMoneyFrameOptions) {
      if (totalSend + amount > maxPacketAmount) {
        throw new Error("Cannot send more money than the maximum packet amount")
      }

      totalSend += amount

      const previousAmount = moneyFrames.get(streamId) ?? 0n

      moneyFrames.set(streamId, previousAmount + amount)

      if (onFulfill) {
        fulfillHandlers.push(onFulfill)
      }

      if (onReject) {
        rejectHandlers.push(onReject)
      }
    },

    closeConnection(options: CloseConnectionFrameOptions) {
      closeConnection = options
    },
    closeStream(streamId: number, options: CloseStreamFrameOptions) {
      if (!closeStreams.has(streamId)) {
        closeStreams.set(streamId, options)
      }
    },

    addStreamResponseHandler(handler: () => void) {
      streamResponseHandlers.push(handler)
    },
    addFulfillHandler(handler: () => void) {
      fulfillHandlers.push(handler)
    },
    addRejectHandler(handler: () => void) {
      rejectHandlers.push(handler)
    },

    /**
     * Find out how much more money can be sent in this prepare packet.
     */
    getAvailableAmount() {
      return maxPacketAmount - totalSend
    },

    getPacket({ exchangeRate }: GetPacketOptions) {
      const frames = new Array<StreamFrame>()

      if (newAddress !== undefined) {
        frames.push({
          type: FrameType.ConnectionNewAddress,
          data: {
            sourceAccount: newAddress,
          },
        })
      }

      if (assetDetails !== undefined) {
        frames.push({
          type: FrameType.ConnectionAssetDetails,
          data: {
            sourceAssetCode: assetDetails.assetCode,
            sourceAssetScale: assetDetails.assetScale,
          },
        })
      }

      for (const [streamId, amount] of moneyFrames) {
        frames.push({
          type: FrameType.StreamMoney,
          data: {
            streamId: BigInt(streamId),
            shares: amount,
          },
        })
      }

      for (const [streamId, options] of closeStreams) {
        frames.push({
          type: FrameType.StreamClose,
          data: {
            streamId: BigInt(streamId),
            errorCode: options.errorCode,
            errorMessage: options.errorMessage,
          },
        })
      }

      if (closeConnection !== undefined) {
        frames.push({
          type: FrameType.ConnectionClose,
          data: {
            errorCode: closeConnection.errorCode,
            errorMessage: closeConnection.errorMessage,
          },
        })
      }

      const destinationAmount = (totalSend * exchangeRate[0]) / exchangeRate[1]

      return {
        sourceAmount: totalSend,
        destinationAmount,
        fulfillable,
        frames,
      }
    },

    callFulfillHandlers() {
      for (const handler of fulfillHandlers) {
        handler()
      }
    },
    callRejectHandlers() {
      for (const handler of rejectHandlers) {
        handler()
      }
    },
    callStreamResponseHandlers() {
      for (const handler of streamResponseHandlers) {
        handler()
      }
    },
  }
}

export type PrepareBuilder = ReturnType<typeof createPrepareBuilder>
