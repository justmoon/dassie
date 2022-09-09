import type { EffectContext } from "@dassie/lib-reactive"

import { walletStore } from "../stores/wallet"
import type { NullUplink } from "../types/account-configuration"

export const maintainUplinks = (sig: EffectContext) => {
  const accounts = sig.get(walletStore, (state) => state.accounts)

  const uplinks = accounts.flatMap((account) => account.uplinks)

  for (const uplink of uplinks) {
    sig.run(maintainUplink, uplink)
  }
}

const maintainUplink = (sig: EffectContext, uplink: NullUplink) => {
  const ws = new WebSocket(uplink.url.replace(/^http/, "ws"))

  ws.addEventListener("message", (event) => {
    console.log("ws message", event)
  })

  sig.onCleanup(() => {
    ws.close()
  })
}
