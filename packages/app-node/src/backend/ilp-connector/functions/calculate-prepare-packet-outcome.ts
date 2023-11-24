import { Reactor } from "@dassie/lib-reactive"
import { UnreachableCaseError, isFailure } from "@dassie/lib-type-utils"

import { applyPacketPrepareToLedger } from "../../accounting/functions/apply-interledger-packet"
import { LedgerStore, Transfer } from "../../accounting/stores/ledger"
import { CalculateOutgoingAmount } from "../../exchange/functions/calculate-outgoing-amount"
import { ResolveIlpAddress } from "../../routing/functions/resolve-ilp-address"
import { MAX_PACKET_AMOUNT } from "../constants/max-packet-amount"
import { AmountTooLargeIlpFailure } from "../failures/amount-too-large-ilp-failure"
import { InsufficientLiquidityIlpFailure } from "../failures/insufficient-liquidity-ilp-failure"
import { InternalErrorIlpFailure } from "../failures/internal-error-ilp-failure"
import { UnreachableIlpFailure } from "../failures/unreachable-ilp-failure"
import { IlpPacket, IlpType } from "../schemas/ilp-packet-codec"
import { EndpointInfo } from "./send-packet"

export interface CalculatePreparePacketOutcomeParameters {
  readonly sourceEndpointInfo: EndpointInfo
  readonly parsedPacket: IlpPacket & { type: typeof IlpType.Prepare }
}

export interface PreparePacketOutcome {
  readonly destinationEndpointInfo: EndpointInfo
  readonly outgoingAmount: bigint
  readonly transfers: Transfer[]
}

export const CalculatePreparePacketOutcome = (reactor: Reactor) => {
  const ledgerStore = reactor.use(LedgerStore)
  const resolveIlpAddress = reactor.use(ResolveIlpAddress)
  const calculateOutgoingAmount = reactor.use(CalculateOutgoingAmount)

  return ({
    sourceEndpointInfo,
    parsedPacket,
  }: CalculatePreparePacketOutcomeParameters):
    | PreparePacketOutcome
    | UnreachableIlpFailure
    | AmountTooLargeIlpFailure
    | InsufficientLiquidityIlpFailure
    | InternalErrorIlpFailure => {
    const destinationEndpointInfo = resolveIlpAddress(
      parsedPacket.data.destination,
    )

    if (isFailure(destinationEndpointInfo)) {
      return new UnreachableIlpFailure("No route found for destination")
    }

    let outgoingAmount = parsedPacket.data.amount
    const transfers: Transfer[] = []
    if (parsedPacket.data.amount > 0n) {
      if (parsedPacket.data.amount > MAX_PACKET_AMOUNT) {
        return new AmountTooLargeIlpFailure(
          "Packet amount exceeds maximum allowed amount",
        )
      }

      outgoingAmount = calculateOutgoingAmount(
        sourceEndpointInfo.accountPath,
        destinationEndpointInfo.accountPath,
        parsedPacket.data.amount,
      )

      const transferParameters = applyPacketPrepareToLedger(
        sourceEndpointInfo.accountPath,
        destinationEndpointInfo.accountPath,
        parsedPacket.data.amount,
        outgoingAmount,
      )

      for (const transfer of transferParameters) {
        const result = ledgerStore.createTransfer(transfer)
        if (isFailure(result)) {
          switch (result.name) {
            case "InvalidAccountFailure": {
              return new InternalErrorIlpFailure(
                "Missing internal ledger account",
              )
            }
            case "ExceedsCreditsFailure": {
              return new InsufficientLiquidityIlpFailure(
                "Insufficient liquidity",
              )
            }
            case "ExceedsDebitsFailure": {
              return new InsufficientLiquidityIlpFailure(
                "Insufficient liquidity",
              )
            }
            default: {
              throw new UnreachableCaseError(result)
            }
          }
        }

        transfers.push(result)
      }
    }

    return {
      destinationEndpointInfo,
      outgoingAmount,
      transfers,
    }
  }
}
