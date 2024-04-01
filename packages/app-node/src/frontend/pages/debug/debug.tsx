import { Route } from "wouter"

import { Database } from "./database/database"
import { AccountDetailPage } from "./ledger/account-detail"
import { Ledger } from "./ledger/ledger"
import { Logs } from "./logs/logs"
import { Nodes } from "./nodes/nodes"
import { Routing } from "./routing/routing"
import { State } from "./state/state"

export const DebugPage = () => {
  return (
    <Route path="/debug" nest>
      <Route path="/logs" component={Logs} />
      <Route path="/nodes" component={Nodes} />
      <Route path="/ledger" component={Ledger} />
      <Route
        path="/ledger/:ledgerId/account/:accountPath+"
        component={AccountDetailPage}
      />
      <Route path="/routing" component={Routing} />
      <Route path="/state" component={State} />
      <Route path="/database" component={Database} />
    </Route>
  )
}
