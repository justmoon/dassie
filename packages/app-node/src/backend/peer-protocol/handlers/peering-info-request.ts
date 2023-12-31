import { DassieReactor } from "../../base/types/dassie-base"
import { ManageSettlementSchemeInstancesActor } from "../../settlement-schemes/manage-settlement-scheme-instances"
import { PeerMessageHandler } from "../functions/handle-peer-message"

export const HandlePeeringInfoRequest = ((reactor: DassieReactor) => {
  return async ({
    message: {
      content: {
        value: {
          value: { settlementSchemeId },
        },
      },
    },
  }) => {
    const settlementActor = reactor
      .use(ManageSettlementSchemeInstancesActor)
      .get(settlementSchemeId)

    if (!settlementActor) {
      throw new Error("Settlement scheme not found")
    }

    const result = await settlementActor.api.getPeeringInfo.ask()

    return {
      settlementSchemeData: result.data,
    }
  }
}) satisfies PeerMessageHandler<"peeringInfoRequest">
