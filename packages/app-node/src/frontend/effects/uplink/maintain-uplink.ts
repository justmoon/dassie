import { BtpContentType, generateBtpMessage } from "@dassie/lib-protocol-utils"
import { createActor } from "@dassie/lib-reactive"

import { walletStore } from "../../stores/wallet"

export const maintainUplink = () =>
  createActor((sig) => {
    const isSeedInitialized = sig.get(walletStore, (state) => !!state.seed)

    if (isSeedInitialized) {
      sig.run(uplinkService, undefined, { register: true })
    }
  })

export interface NodeUplink {
  websocket: WebSocket
}

const handleError = (error: unknown) => {
  console.error("uplink error", error)
}

export const uplinkService = () =>
  createActor<NodeUplink>((sig) => {
    const websocket = new WebSocket(
      `${__DASSIE_NODE_URL__.replace(/^http/, "ws")}btp`
    )

    // eslint-disable-next-line unicorn/consistent-function-scoping
    const handleMessage = (event: MessageEvent) => {
      console.log("ws message", event)
    }

    const handleOpen = () => {
      console.log("ws open")

      const authMessage = generateBtpMessage({
        requestId: 0,
        protocolData: [
          {
            protocolName: "auth",
            contentType: BtpContentType.ApplicationOctetStream,
            data: new Uint8Array(32),
          },
        ],
      })
      websocket.send(authMessage)
    }

    websocket.addEventListener("message", handleMessage)
    websocket.addEventListener("open", handleOpen)
    websocket.addEventListener("error", handleError)

    sig.onCleanup(() => {
      websocket.removeEventListener("message", handleMessage)
      websocket.removeEventListener("open", handleOpen)
      websocket.removeEventListener("error", handleError)
      websocket.close()
    })

    return { websocket }
  })
