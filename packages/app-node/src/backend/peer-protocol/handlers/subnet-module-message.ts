import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import type { SubnetActorFactory } from "../../subnets/types/subnet-module"
import type { IncomingPeerMessageEvent } from "../actions/handle-peer-message"

export const handleSubnetModuleMessage = () =>
  createActor((sig) => {
    const subnetActorMap = new Map<string, ReturnType<SubnetActorFactory>>()

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
            peerKey: `${subnetId}.${sender}`,
            message,
          })
        }

        return EMPTY_UINT8ARRAY
      },
      register: ({
        subnetId,
        subnetActor,
      }: {
        subnetId: string
        subnetActor: SubnetActorFactory
      }) => {
        subnetActorMap.set(subnetId, sig.use(subnetActor))
      },
      deregister: ({ subnetId }: { subnetId: string }) => {
        subnetActorMap.delete(subnetId)
      },
    }
  })
