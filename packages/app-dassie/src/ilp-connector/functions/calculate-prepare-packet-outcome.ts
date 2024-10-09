import {
  type IlpPacket,
  IlpType,
  interledgerTimeToTimestamp,
  timestampToInterledgerTime,
} from "@dassie/lib-protocol-ilp"
import { UnreachableCaseError, isFailure } from "@dassie/lib-type-utils"

import { applyPacketPrepareToLedger } from "../../accounting/functions/apply-interledger-packet"
import { LedgerStore, type Transfer } from "../../accounting/stores/ledger"
import type { DassieReactor } from "../../base/types/dassie-base"
import { CalculateOutgoingAmount } from "../../exchange/functions/calculate-outgoing-amount"
import { ResolveIlpAddress } from "../../routing/functions/resolve-ilp-address"
import {
  ILP_MESSAGE_WINDOW,
  MAXIMUM_HOLD_TIME,
} from "../constants/expiry-constraints"
import { MAX_PACKET_AMOUNT } from "../constants/max-packet-amount"
import { AmountTooLargeIlpFailure } from "../failures/amount-too-large-ilp-failure"
import { InsufficientLiquidityIlpFailure } from "../failures/insufficient-liquidity-ilp-failure"
import { InsufficientTimeoutIlpFailure } from "../failures/insufficient-timeout-ilp-failure"
import { InternalErrorIlpFailure } from "../failures/internal-error-ilp-failure"
import { InvalidPacketIlpFailure } from "../failures/invalid-packet-ilp-failure"
import { UnreachableIlpFailure } from "../failures/unreachable-ilp-failure"
import { GetEndpointAccountPath } from "./get-endpoint-account-path"
import type { EndpointInfo } from "./send-packet"

export interface CalculatePreparePacketOutcomeParameters {
  readonly sourceEndpointInfo: EndpointInfo
  readonly parsedPacket: IlpPacket & { type: typeof IlpType.Prepare }
}

export interface PreparePacketOutcome {
  readonly destinationEndpointInfo: EndpointInfo
  readonly outgoingAmount: bigint
  readonly outgoingExpiry: string
  readonly transfers: Transfer[]
}

// Pre-instantiated failures - saves us a memory allocation when we need to return one of the these
const UNREACHABLE_FAILURE = new UnreachableIlpFailure(
  "No route found for destination",
)
const INSUFFICIENT_TIMEOUT_FAILURE = new InsufficientTimeoutIlpFailure(
  "Insufficient timeout",
)
const AMOUNT_TOO_LARGE_FAILURE = new AmountTooLargeIlpFailure(
  "Packet amount exceeds maximum allowed amount",
)
const MISSING_LEDGER_ACCOUNT_FAILURE = new InternalErrorIlpFailure(
  "Missing internal ledger account",
)
const INSUFFICIENT_LIQUIDITY_FAILURE = new InsufficientLiquidityIlpFailure(
  "Insufficient liquidity",
)

export const CalculatePreparePacketOutcome = (reactor: DassieReactor) => {
  const ledgerStore = reactor.use(LedgerStore)
  const resolveIlpAddress = reactor.use(ResolveIlpAddress)
  const calculateOutgoingAmount = reactor.use(CalculateOutgoingAmount)
  const getEndpointAccountPath = reactor.use(GetEndpointAccountPath)

  function calculatePacketOutcome({
    sourceEndpointInfo,
    parsedPacket,
  }: CalculatePreparePacketOutcomeParameters):
    | PreparePacketOutcome
    | UnreachableIlpFailure
    | AmountTooLargeIlpFailure
    | InsufficientLiquidityIlpFailure
    | InternalErrorIlpFailure
    | InvalidPacketIlpFailure
    | InsufficientTimeoutIlpFailure {
    const destinationEndpointInfo = resolveIlpAddress(
      parsedPacket.data.destination,
    )

    if (isFailure(destinationEndpointInfo)) {
      return UNREACHABLE_FAILURE
    }

    const incomingExpiry = interledgerTimeToTimestamp(
      parsedPacket.data.expiresAt,
    )
    if (isFailure(incomingExpiry)) return incomingExpiry

    const now = reactor.base.clock.now()
    const delta = incomingExpiry - now - ILP_MESSAGE_WINDOW
    if (delta < ILP_MESSAGE_WINDOW) {
      return INSUFFICIENT_TIMEOUT_FAILURE
    }

    const outgoingExpiry = timestampToInterledgerTime(
      now + Math.min(delta, MAXIMUM_HOLD_TIME),
    )

    let outgoingAmount = parsedPacket.data.amount
    const transfers: Transfer[] = []
    if (parsedPacket.data.amount > 0n) {
      if (parsedPacket.data.amount > MAX_PACKET_AMOUNT) {
        return AMOUNT_TOO_LARGE_FAILURE
      }

      outgoingAmount = calculateOutgoingAmount(
        getEndpointAccountPath(sourceEndpointInfo),
        getEndpointAccountPath(destinationEndpointInfo),
        parsedPacket.data.amount,
      )

      const transferParameters = applyPacketPrepareToLedger(
        getEndpointAccountPath(sourceEndpointInfo),
        getEndpointAccountPath(destinationEndpointInfo),
        parsedPacket.data.amount,
        outgoingAmount,
      )

      for (const transfer of transferParameters) {
        const result = ledgerStore.createTransfer(transfer)
        if (isFailure(result)) {
          switch (result.name) {
            case "InvalidAccountFailure": {
              return MISSING_LEDGER_ACCOUNT_FAILURE
            }
            case "ExceedsCreditsFailure":
            case "ExceedsDebitsFailure": {
              return INSUFFICIENT_LIQUIDITY_FAILURE
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
      outgoingExpiry,
      transfers,
    }
  }

  return calculatePacketOutcome
}
