import type { Promisable } from "type-fest"

import type { Actor, Behavior, Factory } from "@dassie/lib-reactive"

import type { VALID_REALMS } from "../../constants/general"
import type { IlpPacketWithAttachedPrepare } from "../../ilp-connector/topics/incoming-ilp-packet"
import type { NodeTableKey } from "../../peer-protocol/stores/node-table"

export interface BalanceMap {
  adjustBalance: (subnetId: string, adjustment: bigint) => void
}

export interface PacketInformation {
  subnetId: string
  balanceMap: BalanceMap
  packet: IlpPacketWithAttachedPrepare
}

export interface SubnetProperties {
  readonly subnetId: string
  host: SubnetHostMethods
}

export interface SettlementParameters {
  amount: bigint
  peerKey: NodeTableKey
}

export interface PeerMessageParameters {
  peerKey: NodeTableKey
  message: Uint8Array
}

export interface SubnetHostMethods {
  sendMessage: (parameters: PeerMessageParameters) => Promisable<void>
}

export interface SubnetActorMethods {
  settle: (parameters: SettlementParameters) => Promisable<void>
  handleMessage: (parameters: PeerMessageParameters) => Promisable<void>
}

export type SubnetActorFactory = Factory<
  Actor<Promisable<SubnetActorMethods>, SubnetProperties>
>

export interface SubnetModule {
  /**
   * The unique subnet identifier.
   *
   * @remarks
   *
   * The subnet identifier is the third segment in the node's ILP address (following `g.das`) and should be unique for each subnet.
   *
   * Per convention, the subnet identifier consists of up to three segments, separated by hyphens. The first segment is the name of the root (layer-1) ledger. The second segment is the name of any overlay technology (such as a layer-2 protocol). The third segment is the settlement currency. When there is an obvious default choice (such as using the ledger's native token as the currency), or when a segment doesn't apply (such as no overlay technology being used), the corresponding segment can be omitted.
   */
  readonly name: string

  /**
   * Which versions of the given subnet protocol this module supports.
   *
   * @remarks
   *
   * This field is used to power a simple version negotiation mechanism. When two peers try to connect and their supported versions don't overlap, the connection is rejected.
   */
  readonly supportedVersions: number[]

  /**
   * Whether this subnet implements real-money settlement.
   *
   * @remarks
   *
   * This field is used to prevent a node from accidentially using both real-money settlement and virtual settlement simultaneously because doing so could enable an attacker to steal funds. Either all subnets must use real-money settlement or all subnets must use virtual settlement.
   */
  readonly realm: (typeof VALID_REALMS)[number]

  /**
   * Behavior of the actor which will be instantiated while the node is connected to this subnet.
   *
   * @remarks
   *
   * Use this to do things like maintaining a connection to the ledger in question.
   */
  behavior: Behavior<Promisable<SubnetActorMethods>, SubnetProperties>
}
