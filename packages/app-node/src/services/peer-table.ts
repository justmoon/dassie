import axios from "axios"
import { produce } from "immer"

import { createLogger } from "@xen-ilp/lib-logger"

import type { Config } from "../config"
import { XenMessage, XenMessageType } from "../protocols/xen/message"
import {
  XenMessageWithOptionalSignature,
  encodeMessage,
} from "../protocols/xen/message"
import type SigningService from "./signing"
import type { Store } from "./state"
import type State from "./state"

const logger = createLogger("xen:node:peer-manager")

export interface PeerTableContext {
  config: Config
  signing: SigningService
  state: State
}

export interface PeerEntry {
  nodeId: string
  url: string
  theirSequence: number
  ourSequence: number
  lastSeen: number
}

export interface NewPeerEntry {
  nodeId: string
  url: string
}

export type Model = Record<string, PeerEntry>

export default class PeerTable {
  readonly store: Store<Model>

  addPeer = (peerEntry: NewPeerEntry) =>
    this.store.set(
      produce((draft) => {
        draft[peerEntry.nodeId] = {
          ...peerEntry,
          theirSequence: 0,
          ourSequence: 0,
          lastSeen: Date.now(),
        }
      })
    )

  updatePeer = (nodeId: string, peerEntry: Partial<PeerEntry>) => {
    this.store.set(
      produce((draft) => {
        const previousEntry = draft[nodeId]
        if (previousEntry == null) {
          throw new Error("nodeId not found")
        }
        draft[nodeId] = {
          ...previousEntry,
          ...peerEntry,
        }
      })
    )
  }

  constructor(readonly context: PeerTableContext) {
    this.store = this.context.state.createStore({})

    for (const peerEntry of this.context.config.initialPeers)
      this.addPeer(peerEntry)

    this.connect()
  }

  get peers() {
    return this.store.get()
  }

  async connect() {
    try {
      await Promise.all(
        Object.values(this.peers).map((peer) => this.sendHello(peer))
      )
    } catch (error) {
      logger.logError(error)
    }
  }

  handleMessage(message: XenMessage) {
    switch (message.method) {
      case XenMessageType.Hello: {
        const { nodeId, sequence, neighbors } = message.signed

        logger.debug("handle hello", {
          from: nodeId,
          sequence,
          neighbors: () =>
            neighbors.map((neighbor) => neighbor.nodeId).join(","),
        })

        const peer = this.peers[nodeId]
        if (peer) {
          if (sequence < peer.theirSequence) {
            logger.debug("ignoring stale hello", { from: nodeId })
            return
          }

          this.updatePeer(nodeId, {
            theirSequence: sequence,
            lastSeen: Date.now(),
          })
        }

        break
      }
      default:
        logger.debug("ignoring unknown message", { method: message.method })
    }
  }

  async sendHello(peer: PeerEntry) {
    logger.debug(`sending hello`, { to: peer.nodeId })

    const ourSequence = peer.ourSequence + 1
    this.updatePeer(peer.nodeId, {
      ourSequence,
    })

    await this.sendMessage(peer, {
      method: XenMessageType.Hello,
      signed: {
        nodeId: this.context.config.nodeId,
        sequence: ourSequence,
        neighbors: Object.values(this.peers).map((peer) => ({
          nodeId: peer.nodeId,
          proof: Buffer.alloc(32),
        })),
      },
    })
  }

  async sendMessage(peer: PeerEntry, message: XenMessageWithOptionalSignature) {
    await axios(`${peer.url}/xen`, {
      method: "POST",
      data: encodeMessage(message, this.context.signing.sign),
      headers: {
        accept: "application/xen-message",
        "content-type": "application/xen-message",
      },
    })
  }
}
