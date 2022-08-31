import type { EffectContext } from "@dassie/lib-reactive"

import type { QueryResponseData } from "../../../app-beacon/src/data-exchange/register-query-http-handler"
import { walletStore } from "../stores/wallet"
import type { AccountConfiguration } from "../types/account-configuration"

export const automaticallyCreateFirstUplinks = async (sig: EffectContext) => {
  const accounts = sig.get(walletStore, (state) => state.accounts)

  for (const account of accounts) {
    if (account.uplinks.length === 0) {
      await sig.run(createFirstUplink, account)
    }
  }
}

export const createFirstUplink = async (
  sig: EffectContext,
  account: AccountConfiguration
) => {
  const responses = await Promise.all(
    __DASSIE_BEACONS__.map(async (beacon) => {
      const result = await fetch(`${beacon}query?subnet=${account.type}`)
      return (await result.json()) as QueryResponseData
    })
  )
  const nodes = responses.flatMap((data) => data.nodes)

  // Use a random node as the first uplink
  sig
    .use(walletStore)
    .addUplink(account, nodes[Math.floor(Math.random() * nodes.length)]!)
}
