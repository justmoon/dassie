import type { Config } from "../config"
import {
  XenMessage,
  XenMessageType,
  XenUnknownMessage,
} from "../protocols/xen/message"
import Peer from "./peer"
import type SigningService from "./signing"

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
      console.error("Error dealing with peers", String(error))
    }
  }

  handleMessage(message: XenMessage | XenUnknownMessage) {
    switch (message.method) {
      case XenMessageType.Hello:
        console.log("handle hello")
        break
      default:
        console.log("handle unknown message")
    }
  }
}
