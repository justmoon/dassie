import {
  ILDCP_ADDRESS,
  ILDCP_CONDITION,
  parseIldcpResponse,
} from "@dassie/lib-protocol-ildcp"
import { IlpType, timestampToInterledgerTime } from "@dassie/lib-protocol-ilp"
import { isFailure } from "@dassie/lib-type-utils"

import type { DassieReactor } from "../../base/types/dassie-base"
import { MAXIMUM_HOLD_TIME } from "../../ilp-connector/constants/expiry-constraints"
import { SendLinkLocalPacket } from "../../ilp-connector/functions/send-link-local-packet"
import type { DassieIlpAddress } from "../../ilp-connector/types/ilp-address"
import UplinkAddressQueryFailure from "../failures/uplink-address-query"
import type { NodeId } from "../types/node-id"

export function QueryUplinkAddress(reactor: DassieReactor) {
  const sendLinkLocalPacket = reactor.use(SendLinkLocalPacket)

  return async function queryUplinkAddress(peerId: NodeId) {
    const ildcpRequestPacket = {
      amount: 0n,
      destination: ILDCP_ADDRESS,
      executionCondition: ILDCP_CONDITION,
      expiresAt: timestampToInterledgerTime(Date.now() + MAXIMUM_HOLD_TIME),
      data: new Uint8Array(),
    }

    const result = await sendLinkLocalPacket({
      packet: ildcpRequestPacket,
      peerId,
    })

    if (result.type === IlpType.Reject) {
      return new UplinkAddressQueryFailure(
        "ILDCP Request Was Rejected: " + result.data.message,
      )
    }

    const ildcpResponse = parseIldcpResponse(result.data.data)

    if (isFailure(ildcpResponse)) {
      return new UplinkAddressQueryFailure(
        "Failed to parse ILDCP response: " + ildcpResponse.message,
      )
    }

    return ildcpResponse.address as DassieIlpAddress
  }
}
