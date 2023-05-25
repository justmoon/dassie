import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { manageSubnetInstances } from "../../subnets/manage-subnet-instances"
import type { IncomingPeerMessageEvent } from "../actors/handle-peer-message"

export const handleSubnetModuleMessage = () =>
  createActor((sig) => {
    const subnetManager = sig.use(manageSubnetInstances)

    return {
      handle: ({
        message: {
          sender,
          content: {
            value: {
              value: { subnetId, message },
            },
          },
        },
      }: IncomingPeerMessageEvent<"subnetModuleMessage">) => {
        subnetManager.tell("handleMessage", {
          subnetId,
          peerId: sender,
          message,
        })

        return EMPTY_UINT8ARRAY
      },
    }
  })
