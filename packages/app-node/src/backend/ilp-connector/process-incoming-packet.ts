import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import {
  processPacketPrepare,
  processPacketResult,
} from "../accounting/functions/process-interledger-packet"
import { ledgerStore } from "../accounting/stores/ledger"
import { IlpPacket, IlpType, parseIlpPacket } from "./ilp-packet-codec"
import { requestIdMapSignal } from "./signals/request-id-map"
import {
  IlpPacketWithAttachedPrepare,
  IncomingIlpPacket,
} from "./topics/incoming-ilp-packet"
import {
  OutgoingIlpPacket,
  outgoingIlpPacketBuffer,
} from "./topics/outgoing-ilp-packet"

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
    const outgoingIlpPacketBufferInstance = sig.use(outgoingIlpPacketBuffer)
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
            logger.debug("received ILP prepare", {
              from: sourceIlpAddress,
              to: packet.destination,
              amount: packet.amount,
            })

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

            const outgoingRequestId = Math.trunc(Math.random() * 0xff_ff_ff_ff)

            requestIdMap.set(outgoingRequestId, {
              sourceAddress: sourceIlpAddress,
              sourceRequestId: requestId,
              preparePacket: packet,
            })

            logger.debug("forwarding ILP prepare", {
              source: sourceIlpAddress,
              destination: packet.destination,
              requestId: outgoingRequestId,
            })

            const outgoingPacketEvent: OutgoingIlpPacket = {
              source: sourceIlpAddress,
              packet,
              asUint8Array: serializedPacket,
              requestId: outgoingRequestId,
              destination: packet.destination,
            }

            outgoingIlpPacketBufferInstance.emit(outgoingPacketEvent)
            break
          }
          case IlpType.Fulfill: {
            logger.debug("received ILP fulfill", {
              requestId,
            })

            if (packet.prepare.amount > 0n) {
              processPacketResult(
                ledger,
                ledgerAccountPath,
                packet.prepare,
                "fulfill"
              )
            }

            const requestMapEntry = requestIdMap.get(requestId)

            if (!requestMapEntry) {
              throw new Error(
                "Received response ILP packet which did not match any request ILP packet we sent"
              )
            }

            requestIdMap.delete(requestId)

            const preparedEvent: OutgoingIlpPacket = {
              source: sourceIlpAddress,
              packet,
              asUint8Array: serializedPacket,
              requestId: requestMapEntry.sourceRequestId,
              destination: requestMapEntry.sourceAddress,
            }

            logger.debug("sending ILP fulfill to source", {
              destination: preparedEvent.destination,
            })

            outgoingIlpPacketBufferInstance.emit(preparedEvent)
            break
          }
          case IlpType.Reject: {
            logger.debug("received ILP reject", {
              triggeredBy: packet.triggeredBy,
              message: packet.message,
              requestId,
            })

            if (packet.prepare.amount > 0n) {
              processPacketResult(
                ledger,
                ledgerAccountPath,
                packet.prepare,
                "reject"
              )
            }

            const requestMapEntry = requestIdMap.get(requestId)

            if (!requestMapEntry) {
              throw new Error(
                "Received response ILP packet which did not match any request ILP packet we sent"
              )
            }

            requestIdMap.delete(requestId)

            const preparedEvent: OutgoingIlpPacket = {
              source: sourceIlpAddress,
              packet,
              asUint8Array: serializedPacket,
              requestId: requestMapEntry.sourceRequestId,
              destination: requestMapEntry.sourceAddress,
            }

            logger.debug("sending ILP reject to source", {
              destination: preparedEvent.destination,
            })

            outgoingIlpPacketBufferInstance.emit(preparedEvent)
            break
          }
        }
      },
    }
  })
