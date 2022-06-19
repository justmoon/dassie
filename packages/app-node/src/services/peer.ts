import axios from "axios"

import { Logger, createLogger } from "@xen-ilp/lib-logger"

import {
  XenMessageType,
  XenMessageWithOptionalSignature,
  encodeMessage,
} from "../protocols/xen/message"
import type { PeerManagerContext } from "./peer-manager"

export default class Peer {
  readonly logger: Logger
  constructor(
    readonly context: PeerManagerContext,
    readonly nodeId: string,
    readonly url: string
  ) {
    this.logger = createLogger(`xen:node:peer:${nodeId}`)
  }

  async sendHello(peers: Array<Peer>) {
    console.log(`sending hello to peer ${this.nodeId}`)

    await this.sendMessage({
      method: XenMessageType.Hello,
      signed: {
        nodeId: this.context.config.nodeId,
        sequence: 0,
        neighbors: peers.map((peer) => ({
          nodeId: peer.nodeId,
          proof: Buffer.alloc(32),
        })),
      },
    })
  }

  async sendMessage(message: XenMessageWithOptionalSignature) {
    try {
      await axios(`${this.url}/xen`, {
        method: "POST",
        data: encodeMessage(message, this.context.signing.sign),
        headers: {
          accept: "application/xen-message",
          "content-type": "application/xen-message",
        },
      })
    } catch (error) {
      if (axios.isAxiosError(error) && error.response) {
        console.error(
          `Error sending message to peer ${this.nodeId}: ${error.response.status} ${error.response.statusText} ${error.response.data}`
        )
      } else {
        throw error
      }
    }
  }
}