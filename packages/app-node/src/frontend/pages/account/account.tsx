import { Link } from "wouter"

import { Amount } from "../../components/amount/amount"
import { Button } from "../../components/ui/button"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "../../components/ui/card"
import { useAccount } from "../../hooks/use-account"
import { trpc } from "../../utils/trpc"

export const Account = () => {
  const account = useAccount()
  const { data: basicState } = trpc.general.getBasicState.useQuery()

  return (
    <div className="flex flex-col items-center gap-4 p-4">
      <Card>
        <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
          <CardTitle className="text-sm font-medium">Payment Pointer</CardTitle>
        </CardHeader>
        <CardContent className="text-lg font-bold">
          {basicState ? `$${basicState.hostname}` : "???"}
        </CardContent>
      </Card>
      <Card>
        <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
          <CardTitle className="text-sm font-medium">Balance</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="flex p-2 text-center max-w-md w-full justify-center text-4xl md:text-5xl">
            <Amount value={account.balance} currency={account.currency} />
          </div>
        </CardContent>
      </Card>
      <Link href="/send">
        <Button>Send</Button>
      </Link>
    </div>
  )
}
