import type { Config } from "../config"
import type { type as XenMessage } from "../protocols/xen/message"
import Peer from "./peer"

export interface PeerManagerContext {
  config: Config
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
      console.error("Error dealing with peers", String(error))
    }
  }

  handleMessage(message: XenMessage) {
    switch (message.method) {
      case "hello":
        console.log("handle hello")
    }
  }
}
