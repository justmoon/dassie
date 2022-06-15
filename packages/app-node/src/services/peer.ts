import axios from "axios"

import type { type as XenMessage } from "../protocols/xen/message"
import type { PeerManagerContext } from "./peer-manager"

export default class Peer {
  constructor(
    readonly context: PeerManagerContext,
    readonly nodeId: string,
    readonly url: string
  ) {}

  async sendHello(peers: Array<Peer>) {
    console.log(`sending hello to peer ${this.nodeId}`)

    await this.sendMessage({
      method: "hello",
      params: {
        nodeId: this.context.config.nodeId,
        sequence: 0,
        neighbors: peers.map((peer) => ({ nodeId: peer.nodeId, proof: "" })),
      },
    })
  }

  async sendMessage(message: XenMessage) {
    try {
      const response = await axios(`${this.url}/xen`, {
        method: "POST",
        data: message,
      })
      console.log(response.data)
    } catch (error) {
      if (axios.isAxiosError(error) && error.response) {
        console.error(
          `Error sending message to peer ${this.nodeId}: ${error.response.status} ${error.response.statusText} ${error.response.data}`
        )
      }
    }
  }
}
