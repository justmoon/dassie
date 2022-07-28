import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import { configStore } from "../config"
import { outgoingUnsignedXenMessageTopic } from "../xen-protocol/topics/xen-protocol"
import { PeerEntry, peerTableStore } from "./stores/peer-table"

const logger = createLogger("xen:node:peer-greeter")

const MAX_GREETING_INTERVAL = 20_000

export const compareSetOfKeys = (
  a: Map<string, unknown>,
  b: Map<string, unknown>
) => {
  if (a.size !== b.size) return false

  for (const key of a.keys()) {
    if (!b.has(key)) return false
  }

  return true
}

export const peerGreeter = (sig: EffectContext) => {
  // Get the current peers and re-run the effect iff the IDs of the peers change.
  const peers = sig.get(
    peerTableStore,
    (peerTable) => peerTable,
    compareSetOfKeys
  )

  const { nodeId, port } = sig.get(configStore, ({ nodeId, port }) => ({
    nodeId,
    port,
  }))

  const sendHello = (peer: PeerEntry) => {
    const sequence = BigInt(Date.now())

    logger.debug(`sending hello`, { to: peer.nodeId, sequence })

    sig.emit(outgoingUnsignedXenMessageTopic, {
      destination: peer.nodeId,
      envelope: {
        nodeId,
        message: {
          hello: {
            sequence,
            url: `https://${nodeId}.localhost:${port}`,
            neighbors: [...peers.values()].map((peer) => ({
              nodeId: peer.nodeId,
              proof: new Uint8Array(32),
            })),
          },
        },
      },
    })
  }

  let timer: NodeJS.Timer | undefined = undefined
  const tick = () => {
    for (const peer of peers.values()) {
      sendHello(peer)
    }

    // Schedule the next tick. Intervals are uniformly distributed between 0 and the maximum greeting interval.
    timer = setTimeout(tick, Math.random() * MAX_GREETING_INTERVAL)
  }

  tick()

  sig.onCleanup(() => {
    if (timer) clearTimeout(timer)
  })
}
