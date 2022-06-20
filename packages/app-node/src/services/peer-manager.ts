import { createLogger } from "@xen-ilp/lib-logger"

import type { Config } from "../config"
import { XenMessage, XenMessageType } from "../protocols/xen/message"
import Peer from "./peer"
import type SigningService from "./signing"

const logger = createLogger("xen:node:peer-manager")

export interface PeerManagerContext {
  config: Config
  signing: SigningService
}

export default class PeerManager {
  readonly peers: Array<Peer>

  constructor(readonly context: PeerManagerContext) {
    this.peers = this.context.config.initialPeers.map(
      ({ nodeId, url }) => new Peer(this.context, nodeId, url)
    )
    this.connect()
  }

  async connect() {
    try {
      await Promise.all(this.peers.map((peer) => peer.sendHello(this.peers)))
    } catch (error) {
      logger.logError(error)
    }
  }

  handleMessage(message: XenMessage) {
    switch (message.method) {
      case XenMessageType.Hello:
        logger.debug("handle hello", {
          from: message.signed.nodeId,
          sequence: message.signed.sequence,
          neighbors: () =>
            message.signed.neighbors
              .map((neighbor) => neighbor.nodeId)
              .join(","),
        })
        break
      default:
        logger.debug("ignoring unknown message", { method: message.method })
    }
  }
}
