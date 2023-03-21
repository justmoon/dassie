import axios from "axios"

import { createLogger } from "@dassie/lib-logger"
import { ia5String, sequence, sequenceOf } from "@dassie/lib-oer"
import { createActor } from "@dassie/lib-reactive"

import { configSignal } from "../config"

const logger = createLogger("das:node:ping-beacons")

const pingSchema = sequence({
  nodeId: ia5String(),
  url: ia5String(),
  subnets: sequenceOf(
    sequence({
      subnetId: ia5String(),
    })
  ),
})

const sendPingToBeacon = async (beaconUrl: string, message: Uint8Array) => {
  await axios(`${beaconUrl}/ping`, {
    method: "POST",
    data: message,
    headers: {
      "content-type": "application/dassie-beacon-ping",
    },
  })
}

export const pingBeacons = () =>
  createActor((sig) => {
    const beacons = sig.get(configSignal, (config) => config.beacons)
    const nodeId = sig.get(configSignal, (config) => config.nodeId)
    const port = sig.get(configSignal, (config) => config.port)

    const url = `https://${nodeId}.localhost:${port}`

    const pingSerializeResult = pingSchema.serialize({
      nodeId,
      url,
      subnets: [{ subnetId: "null" }],
    })

    if (!pingSerializeResult.success) {
      logger.error("failed to serialize beacon ping", {
        error: pingSerializeResult.error,
      })
      return
    }

    Promise.all(
      beacons.map(({ url }) => sendPingToBeacon(url, pingSerializeResult.value))
    ).catch((error: unknown) => {
      logger.error("failed to send beacon ping", { error })
    })
  })
