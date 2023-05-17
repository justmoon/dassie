import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import {
  processPacketPrepare,
  processPacketResult,
} from "../accounting/functions/process-interledger-packet"
import { ledgerStore } from "../accounting/stores/ledger"
import { IlpPacket, IlpType, parseIlpPacket } from "./ilp-packet-codec"
import { requestIdMapSignal } from "./route-ilp-packets"
import {
  IlpPacketWithAttachedPrepare,
  IncomingIlpPacket,
  incomingIlpPacketTopic,
} from "./topics/incoming-ilp-packet"

const logger = createLogger("das:ilp-connector:process-incoming-packet")

export interface ProcessIncomingPacketParameters {
  sourceIlpAddress: string
  ledgerAccountPath: string
  serializedPacket: Uint8Array
  parsedPacket?: IlpPacket | undefined
  requestId: number
}

export const processIncomingPacket = () =>
  createActor((sig) => {
    const ledger = sig.use(ledgerStore)
    const incomingIlpPacketTopicValue = sig.use(incomingIlpPacketTopic)
    const requestIdMap = sig.use(requestIdMapSignal).read()

    const lookupPacket = (
      packet: IlpPacket,
      requestId: number
    ): IlpPacketWithAttachedPrepare => {
      if (packet.type === IlpType.Prepare) {
        return packet
      } else {
        const requestMapEntry = requestIdMap.get(requestId)
        if (!requestMapEntry) {
          throw new Error(
            "Received response ILP packet which did not match any request ILP packet we sent"
          )
        }

        return {
          ...packet,
          prepare: requestMapEntry.preparePacket,
        }
      }
    }

    return {
      handle: ({
        sourceIlpAddress,
        ledgerAccountPath,
        serializedPacket,
        requestId,
        parsedPacket: optionalParsedPacket,
      }: ProcessIncomingPacketParameters) => {
        logger.debug("handle interledger packet", {
          from: sourceIlpAddress,
        })

        // Parse packet if not already done
        const parsedPacket =
          optionalParsedPacket ?? parseIlpPacket(serializedPacket)
        const incomingPacketEvent: IncomingIlpPacket = {
          source: sourceIlpAddress,
          asUint8Array: serializedPacket,
          packet: lookupPacket(parsedPacket, requestId),
          requestId: requestId,
        }

        const { packet } = incomingPacketEvent

        switch (packet.type) {
          case IlpType.Prepare: {
            if (packet.amount > 0n) {
              const result = processPacketPrepare(
                ledger,
                ledgerAccountPath,
                packet,
                "incoming"
              )
              if (isFailure(result)) {
                // TODO: reject packet
                throw new Error(
                  `failed to create transfer, invalid ${result.whichAccount} account: ${result.accountPath}`
                )
              }
            }
            break
          }
          case IlpType.Fulfill: {
            if (packet.prepare.amount > 0n) {
              processPacketResult(
                ledger,
                ledgerAccountPath,
                packet.prepare,
                "fulfill"
              )
            }
            break
          }
          case IlpType.Reject: {
            if (packet.prepare.amount > 0n) {
              processPacketResult(
                ledger,
                ledgerAccountPath,
                packet.prepare,
                "reject"
              )
            }
            break
          }
        }

        incomingIlpPacketTopicValue.emit(incomingPacketEvent)
      },
    }
  })
