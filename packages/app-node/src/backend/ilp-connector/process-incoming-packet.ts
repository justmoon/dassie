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
  PreparedIlpPacketEvent,
  preparedIlpPacketTopic,
} from "./topics/prepared-ilp-packet"
import {
  ResolvedIlpPacketEvent,
  resolvedIlpPacketTopic,
} from "./topics/resolved-ilp-packet"

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
    const preparedIlpPacketTopicValue = sig.use(preparedIlpPacketTopic)
    const resolvedIlpPacketTopicValue = sig.use(resolvedIlpPacketTopic)
    const requestIdMap = sig.use(requestIdMapSignal).read()

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

        if (parsedPacket.type === IlpType.Prepare) {
          logger.debug("received ILP prepare", {
            from: sourceIlpAddress,
            to: parsedPacket.destination,
            amount: parsedPacket.amount,
          })

          if (parsedPacket.amount > 0n) {
            const result = processPacketPrepare(
              ledger,
              ledgerAccountPath,
              parsedPacket,
              "incoming"
            )
            if (isFailure(result)) {
              // TODO: reject packet
              throw new Error(
                `failed to create transfer, invalid ${result.whichAccount} account ${result.accountPath}`
              )
            }
          }

          const outgoingRequestId = Math.trunc(Math.random() * 0xff_ff_ff_ff)

          const preparedIlpPacketEvent: PreparedIlpPacketEvent = {
            sourceIlpAddress,
            ledgerAccountPath,
            serializedPacket,
            parsedPacket,
            incomingRequestId: requestId,
            outgoingRequestId,
          }

          requestIdMap.set(outgoingRequestId, preparedIlpPacketEvent)

          logger.debug("forwarding ILP prepare", {
            source: sourceIlpAddress,
            destination: parsedPacket.destination,
            requestId: outgoingRequestId,
          })

          preparedIlpPacketTopicValue.emit(preparedIlpPacketEvent)
        } else {
          const fulfillOrReject =
            parsedPacket.type === IlpType.Fulfill ? "fulfill" : "reject"
          logger.debug(`received ILP ${fulfillOrReject}`, {
            requestId,
          })

          const prepare = requestIdMap.get(requestId)

          if (!prepare) {
            throw new Error(
              "Received response ILP packet which did not match any request ILP packet we sent"
            )
          }

          requestIdMap.delete(requestId)

          if (prepare.parsedPacket.amount > 0n) {
            processPacketResult(
              ledger,
              ledgerAccountPath,
              prepare.parsedPacket,
              fulfillOrReject
            )
          }

          const resolvedIlpPacketEvent: ResolvedIlpPacketEvent = {
            prepare,
            parsedPacket,
            serializedPacket,
          }

          logger.debug(`sending ILP ${fulfillOrReject} to source`, {
            destination: prepare.sourceIlpAddress,
          })

          resolvedIlpPacketTopicValue.emit(resolvedIlpPacketEvent)
        }
      },
    }
  })
