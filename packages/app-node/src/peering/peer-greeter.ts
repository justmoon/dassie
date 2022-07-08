import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import { configStore } from "../config"
import { XenMessageType } from "../xen-protocol/codecs/xen-message"
import { outgoingUnsignedXenMessageTopic } from "../xen-protocol/topics/xen-protocol"
import { PeerEntry, peerTableStore, updatePeer } from "./stores/peer-table"

const logger = createLogger("xen:node:peer-greeter")

const MAX_GREETING_INTERVAL = 20_000

export const compareSetOfKeys = (
  a: Record<string, unknown>,
  b: Record<string, unknown>
) => {
  const aKeys = Object.keys(a)
  const bKeys = Object.keys(b)

  // A map would be a lot faster here.
  // See: https://stackoverflow.com/a/58492580
  if (aKeys.length !== bKeys.length) return false

  for (const key of aKeys) {
    if (!(key in b)) return false
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

  const nodeId = sig.get(configStore, (config) => config.nodeId)

  const sendHello = (peer: PeerEntry) => {
    logger.debug(`sending hello`, { to: peer.nodeId })

    // Increment our greeting sequence number.
    const ourSequence = peer.ourSequence + 1
    sig.reactor.emit(
      peerTableStore,
      updatePeer(peer.nodeId, {
        ourSequence,
      })
    )

    sig.reactor.emit(outgoingUnsignedXenMessageTopic, {
      destination: peer.nodeId,
      message: {
        method: XenMessageType.Hello,
        signed: {
          nodeId,
          sequence: ourSequence,
          neighbors: Object.values(peers).map((peer) => ({
            nodeId: peer.nodeId,
            proof: Buffer.alloc(32),
          })),
        },
      },
    })
  }

  let timer: NodeJS.Timer | undefined = undefined
  const tick = () => {
    for (const peer of Object.values(peers)) {
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
