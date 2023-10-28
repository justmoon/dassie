import type { Promisable } from "type-fest"

import type { Actor, Behavior } from "@dassie/lib-reactive"

import type { VALID_REALMS } from "../../constants/general"
import { NodeId } from "../../peer-protocol/types/node-id"
import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"

export interface BalanceMap {
  adjustBalance: (
    settlementSchemeId: SettlementSchemeId,
    adjustment: bigint,
  ) => void
}

export interface SettlementSchemeProperties {
  readonly settlementSchemeId: SettlementSchemeId
  host: SettlementSchemeHostMethods
}

export interface OutgoingSettlementParameters {
  amount: bigint
  peerId: NodeId
}

export interface OutgoingSettlementResult {
  proof: Uint8Array
}

export interface IncomingSettlementParameters {
  amount: bigint
  peerId: NodeId
  proof: Uint8Array
}

export interface IncomingSettlementResult {
  result: "accept" | "reject"
}

export interface PeerMessageParameters {
  peerId: NodeId
  message: Uint8Array
}

export interface SettlementSchemeHostMethods {
  sendMessage: (parameters: PeerMessageParameters) => Promisable<void>
}

export interface SettlementSchemeActorMethods {
  settle: (
    parameters: OutgoingSettlementParameters,
  ) => Promisable<OutgoingSettlementResult>
  handleSettlement: (
    parameters: IncomingSettlementParameters,
  ) => Promisable<IncomingSettlementResult>
  handleMessage: (parameters: PeerMessageParameters) => Promisable<void>
}

export type SettlementSchemeActor = Actor<
  Promisable<SettlementSchemeActorMethods>,
  SettlementSchemeProperties
>

export interface SettlementSchemeModule {
  /**
   * The unique settlement scheme identifier.
   *
   * @remarks
   *
   * The settlement scheme identifier is the third segment in the node's ILP address (following `g.das`) and should be unique for each settlement scheme.
   *
   * Per convention, the settlement scheme identifier consists of up to three segments, separated by hyphens. The first segment is the name of the root (layer-1) ledger. The second segment is the name of any overlay technology (such as a layer-2 protocol). The third segment is the settlement currency. When there is an obvious default choice (such as using the ledger's native token as the currency), or when a segment doesn't apply (such as no overlay technology being used), the corresponding segment can be omitted.
   */
  readonly name: string

  /**
   * Which versions of the given settlement scheme protocol this module supports.
   *
   * @remarks
   *
   * This field is used to power a simple version negotiation mechanism. When two peers try to connect and their supported versions don't overlap, the connection is rejected.
   */
  readonly supportedVersions: number[]

  /**
   * Whether this settlement scheme implements real-money settlement.
   *
   * @remarks
   *
   * This field is used to prevent a node from accidentially using both real-money settlement and virtual settlement simultaneously because doing so could enable an attacker to steal funds. Either all settlement schemes must use real-money settlement or all settlement schemes must use virtual settlement.
   */
  readonly realm: (typeof VALID_REALMS)[number]

  /**
   * Behavior of the actor which will be instantiated while the node is connected to this settlement scheme.
   *
   * @remarks
   *
   * Use this to do things like maintaining a connection to the ledger in question.
   */
  behavior: Behavior<
    Promisable<SettlementSchemeActorMethods>,
    SettlementSchemeProperties
  >
}
