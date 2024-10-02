import type { Promisable } from "type-fest"

import type { LedgerId } from "../../accounting/constants/ledgers"
import type { DassieActorContext } from "../../base/types/dassie-base"
import type { VALID_REALMS } from "../../constants/general"
import type { NodeId } from "../../peer-protocol/types/node-id"
import type { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"

export interface SettlementSchemeHostMethods {
  sendMessage: (parameters: {
    peerId: NodeId
    message: Uint8Array
  }) => Promisable<void>

  reportIncomingSettlement: (parameters: {
    ledgerId: LedgerId
    peerId: NodeId
    amount: bigint
  }) => void

  finalizeOutgoingSettlement: (parameters: { settlementId: string }) => void

  cancelOutgoingSettlement: (parameters: { settlementId: string }) => void

  reportDeposit: (parameters: { ledgerId: LedgerId; amount: bigint }) => void

  reportWithdrawal: (parameters: { ledgerId: LedgerId; amount: bigint }) => void

  getEntropy: (parameters: { path: string }) => Uint8Array
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
   * Prepare a settlement transaction.
   *
   * @remarks
   *
   * The settlement method should prepare a settlement transaction, calculate the expected fee, and generate a unique
   * identifier for the settlement. The settlement transaction should not be submitted to the ledger yet.
   *
   * The method must return an execute function which will submit the transaction to the ledger.
   *
   * The method must also return a unique identifier for the settlement. This identifier must be unique per settlement
   * scheme id.
   *
   * When the settlement is finally executed, the settlement scheme module must call `finalizeOutgoingSettlement`. If
   * the settlement fails, the settlement scheme module must call `cancelOutgoingSettlement` but only once the failure
   * is guaranteed to be final. Both of these methods take the `settlementId` to identify the settlement.
   */
  prepareSettlement: (parameters: {
    amount: bigint
    peerId: NodeId
    peerState: TPeerState
  }) => Promisable<{
    message: Uint8Array
    peerState?: TPeerState | undefined
    settlementId: string
    execute: () => Promisable<{
      peerState?: TPeerState | undefined
    }>
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
    settlementSchemeData: Uint8Array
    peerState: TPeerState
  }) => Promisable<void>

  /**
   * Handle an incoming message from the peer.
   *
   * @param parameters - The peer ID and message.
   */
  handleMessage: (parameters: {
    peerId: NodeId
    message: Uint8Array
  }) => Promisable<void>

  /**
   * Process an incoming deposit by the owner.
   */
  handleDeposit: (parameters: { amount: bigint }) => Promisable<void>

  getBalance: () => bigint
}

export interface SettlementSchemeBehaviorParameters {
  sig: DassieActorContext
  settlementSchemeId: SettlementSchemeId
  host: SettlementSchemeHostMethods
}

export interface SettlementSchemeModule<TPeerState extends object = object> {
  /**
   * The unique settlement scheme identifier.
   *
   * @remarks
   *
   * The settlement scheme identifier should be unique for each settlement scheme.
   *
   * Per convention, the settlement scheme identifier can consist of multiple elements separated by plus (+) characters. The first element should be the most general, such as the type of settlement scheme, followed by more specific elements such as the specific ledger instance, overlay network (layer-2+) technology, and settlement currency. For example, `btc+taproot+usd` could refer to a settlement scheme which uses the Bitcoin blockchain and the Taproot overlay network to settle payments in the US Dollar.
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
   * Reference to the internal ledger that should be used for this settlement scheme.
   */
  readonly ledger: LedgerId

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
