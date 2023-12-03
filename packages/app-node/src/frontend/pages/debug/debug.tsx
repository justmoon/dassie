import { Route, Router, useRouter } from "wouter"

import { Database } from "./database/database"
import { AccountDetailPage } from "./ledger/account-detail"
import { Ledger } from "./ledger/ledger"
import { Nodes } from "./nodes/nodes"
import { Routing } from "./routing/routing"
import { State } from "./state/state"

export const DebugPage = () => {
  const router = useRouter()

  return (
    <Router base="/debug" parent={router}>
      <Route path="/nodes" component={Nodes} />
      <Route path="/ledger" component={Ledger} />
      <Route
        path="/ledger/:ledgerId/account/:accountPath+"
        component={AccountDetailPage}
      />
      <Route path="/routing" component={Routing} />
      <Route path="/state" component={State} />
      <Route path="/database" component={Database} />
    </Router>
  )
}
