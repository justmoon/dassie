import type { Promisable } from "type-fest"

import type { VALID_REALMS } from "../../constants/general"

export interface SettlementRequest {
  recipient: string
  amount: bigint

  /**
   * Node identifier
   */
  nodeId: string

  /**
   * Unique sequence number for this settlement request.
   */
  sequence: number
}

export interface SettlementInfo extends SettlementRequest {
  canonicalTransactionId: string
}

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
  readonly realm: typeof VALID_REALMS[number]

  /**
   * Returns the node's own address within the underlying settlement mechanism.
   */
  getAddress(): Promisable<string>

  /**
   * Send money to the recipient.
   *
   * @returns A unique and canonical transaction ID.
   */
  sendMoney(settlementRequest: SettlementRequest): Promise<SettlementInfo>

  /**
   * Verify that a given settlement was indeed received.
   *
   * @remarks
   *
   * **WARNING** It's very easy to implement this function incorrectly which will lead to **loss of funds** as it may allow attackers to submit fake transactions, lie about the amount delivered, claim credit for the same settlement transaction multiple times, etc.
   *
   * This method must verify that:
   *
   *  1. The settlement transaction is valid.
   *  2. The settlement transaction was correctly signed by the sender.
   *  3. The settlement was executed successfully.
   *  4. The settlement transaction has been added to the ledger irrevesibly.
   *  5. The funds were delivered to the correct recipient.
   *  6. The claimed amount was actually delivered.
   *  7. The settlement transaction took place on a live network with real funds.
   *  8. The sender's nodeId was included as data in the signed portion of the transaction.
   *  9. The sequence number was included as data in the signed portion of the transaction.
   *
   * @param settlement - A unique transaction ID.
   */
  verifyIncomingTransaction(settlement: SettlementInfo): Promise<boolean>
}

/**
 * Constructs a subnet module.
 */
export type Subnet = () => Promisable<SubnetModule>
