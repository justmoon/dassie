import type { Promisable } from "type-fest"

import type { ActorContext } from "@dassie/lib-reactive"

import { LedgerId } from "../../accounting/types/ledger-id"
import type { VALID_REALMS } from "../../constants/general"
import { CurrencyDescription } from "../../exchange/load-exchange-rates"
import { NodeId } from "../../peer-protocol/types/node-id"
import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"

export interface SettlementSchemeHostMethods {
  sendMessage: (parameters: {
    peerId: NodeId
    message: Uint8Array
  }) => Promisable<void>

  reportOnLedgerBalance: (parameters: {
    ledgerId: LedgerId
    balance: bigint
  }) => void
}

export interface SettlementSchemeActorMethods<
  TPeerState extends object = object,
> {
  /**
   * Generate information about peering with this peer using this settlement scheme.
   *
   * @returns A blob of data which provides the information necessary to generate a peering request to this peer.
   */
  getPeeringInfo: () => Promisable<{
    data: Uint8Array
  }>

  /**
   * Create a blob of data which can be sent to a peer along with a peering request.
   *
   * @remarks
   *
   * Typically, this would include information that the peer could use to verify that we are indeed an account holder
   * on the ledger that we propose as the settlement scheme. The peer may also check that we aren't blocklisted, etc.
   *
   * @returns A binary blob of data to include with the peering request.
   */
  createPeeringRequest: (parameters: {
    peerId: NodeId
    peeringInfo: Uint8Array
  }) => Promisable<{
    data: Uint8Array
  }>

  /**
   * Process an incoming peering request.
   *
   * @remarks
   *
   * This method is called when a peer sends us a peering request. The data that the peer included is passed in via the
   * `data` parameter. The peer's node ID is passed in via the `peerId` parameter.
   *
   * @returns A response providing information back to the requesting peer or false if the request should be rejected.
   */
  acceptPeeringRequest: (parameters: {
    peerId: NodeId
    data: Uint8Array
  }) => Promisable<
    | {
        peeringResponseData: Uint8Array
        peerState: TPeerState
      }
    | false
  >

  /**
   * Process the response to a successful peering request we made.
   *
   * @remarks
   *
   * If the peer accepted our peering request, they have the option to send us a blob of data back. This method is
   * called to process that data.
   */
  finalizePeeringRequest: (parameters: {
    peerId: NodeId
    peeringInfo: Uint8Array
    data: Uint8Array
  }) => Promisable<{
    peerState: TPeerState
  }>

  /**
   * Initiate a settlement.
   *
   * @param parameters - The amount to settle and the peer to settle with.
   * @returns Proof of settlement which may be relayed to the peer.
   */
  settle: (parameters: {
    amount: bigint
    peerId: NodeId
    peerState: TPeerState
  }) => Promisable<{
    proof: Uint8Array
    peerState?: TPeerState | undefined
  }>

  /**
   * Process an incoming settlement.
   *
   * @param parameters - The peer ID, amount, and provided proof of settlement.
   * @returns A result indicating whether the settlement was accepted or rejected.
   */
  handleSettlement: (parameters: {
    amount: bigint
    peerId: NodeId
    proof: Uint8Array
    peerState: TPeerState
  }) => Promisable<{
    result: "accept" | "reject"
    peerState?: TPeerState | undefined
  }>

  /**
   * Handle an incoming message from the peer.
   *
   * @param parameters - The peer ID and message.
   */
  handleMessage: (parameters: {
    peerId: NodeId
    message: Uint8Array
  }) => Promisable<void>
}

export interface SettlementSchemeBehaviorParameters {
  sig: ActorContext
  settlementSchemeId: SettlementSchemeId
  host: SettlementSchemeHostMethods
}

export interface SettlementSchemeModule<TPeerState extends object = object> {
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
   * Configuration for the internal ledger related to the settlement scheme.
   */
  readonly ledger: {
    readonly id: LedgerId
    readonly currency: CurrencyDescription
  }

  /**
   * Behavior of the actor which will be instantiated while the node is connected to this settlement scheme.
   *
   * @remarks
   *
   * Use this to do things like maintaining a connection to the ledger in question.
   */
  behavior: (
    parameters: SettlementSchemeBehaviorParameters,
  ) => Promisable<SettlementSchemeActorMethods<TPeerState>>
}
