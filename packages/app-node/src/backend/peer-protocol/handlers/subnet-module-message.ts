import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import type { SubnetActorFactory } from "../../subnets/types/subnet-module"
import type { IncomingPeerMessageEvent } from "../actors/handle-peer-message"
import { SubnetId } from "../types/subnet-id"

export const handleSubnetModuleMessage = () =>
  createActor((sig) => {
    const subnetActorMap = new Map<SubnetId, ReturnType<SubnetActorFactory>>()

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
        const subnetActorInstance = subnetActorMap.get(subnetId)

        if (subnetActorInstance) {
          subnetActorInstance.tell("handleMessage", {
            peerId: sender,
            message,
          })
        }

        return EMPTY_UINT8ARRAY
      },
      register: ({
        subnetId,
        subnetActor,
      }: {
        subnetId: SubnetId
        subnetActor: SubnetActorFactory
      }) => {
        subnetActorMap.set(subnetId, sig.use(subnetActor))
      },
      deregister: ({ subnetId }: { subnetId: SubnetId }) => {
        subnetActorMap.delete(subnetId)
      },
    }
  })
