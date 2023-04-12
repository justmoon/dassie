import { createActor } from "@dassie/lib-reactive"

import { handlePeerMessage } from "./actions/handle-peer-message"
import { sendPeerMessage } from "./actions/send-peer-message"
import { peersArrayComputation, peersComputation } from "./computed/peers"
import { requestedPeersComputation } from "./computed/requested-peers"
import { discoverNodes } from "./discover-nodes"
import { forwardLinkStateUpdate } from "./forward-link-state-update"
import { registerPeerHttpHandler } from "./register-peer-http-handler"
import { runPerPeerEffects } from "./run-per-peer-effects"
import { sendHeartbeats } from "./send-heartbeats"

export const speakPeerProtocol = () =>
  createActor((sig) => {
    sig.run(handlePeerMessage, undefined, { register: true })
    sig.run(sendPeerMessage, undefined, { register: true })
    sig.run(peersComputation, undefined, { register: true })
    sig.run(requestedPeersComputation, undefined, { register: true })

    // Handle incoming Dassie messages via HTTP
    sig.run(registerPeerHttpHandler)

    sig.run(sendHeartbeats)
    sig.run(forwardLinkStateUpdate)
    sig.run(discoverNodes)

    sig.for(peersArrayComputation, runPerPeerEffects)
  })
