import { EffectContext, createService } from "@dassie/lib-reactive"

import { walletStore } from "../../stores/wallet"

export const maintainUplink = (sig: EffectContext) => {
  const isSeedInitialized = sig.get(walletStore, (state) => !!state.seed)

  if (isSeedInitialized) {
    sig.run(sig.use(uplinkService).effect)
  }
}

export interface NodeUplink {
  websocket: WebSocket
}

export const uplinkService = () =>
  createService<NodeUplink>((sig) => {
    const websocket = new WebSocket(__DASSIE_NODE_URL__.replace(/^http/, "ws"))

    websocket.addEventListener("message", (event) => {
      console.log("ws message", event)
    })

    websocket.addEventListener("open", () => {
      console.log("ws open")
      websocket.send("hello")
    })

    sig.onCleanup(() => {
      websocket.close()
    })

    return undefined
  })
