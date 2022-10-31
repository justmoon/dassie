import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { createWSClient, wsLink } from "@trpc/react-query"
import { useState } from "react"
import { Route, Switch } from "wouter"

import { useSig } from "@dassie/lib-reactive-react"

import { rootEffect } from "./effects/root"
import { Account } from "./pages/account/account"
import { CreateFirstAccount } from "./pages/create-first-account/create-first-account"
import { Open } from "./pages/open/open"
import { PaymentStatus } from "./pages/payment-status/payment-status"
import { Send } from "./pages/send/send"
import { walletStore } from "./stores/wallet"
import { trpc } from "./utils/trpc"

const App = () => {
  const [queryClient] = useState(() => new QueryClient())
  const [trpcClient] = useState(() => {
    const wsClient = createWSClient({
      url: `${__DASSIE_NODE_URL__.replace(/^http/, "ws")}trpc`,
    })
    return trpc.createClient({
      links: [wsLink({ client: wsClient })],
    })
  })

  const sig = useSig()
  sig.run(rootEffect)

  const wallet = sig.get(walletStore)

  if (wallet.seed == undefined) {
    return <Open />
  }

  if (wallet.accounts.length === 0) {
    return <CreateFirstAccount />
  }

  return (
    <trpc.Provider client={trpcClient} queryClient={queryClient}>
      <QueryClientProvider client={queryClient}>
        <Switch>
          <Route path="/send" component={Send} />
          <Route path="/payments/:paymentId" component={PaymentStatus} />
          <Route component={Account} />
        </Switch>
      </QueryClientProvider>
    </trpc.Provider>
  )
}

export default App
