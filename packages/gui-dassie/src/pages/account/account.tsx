import { DownloadIcon, UploadIcon } from "lucide-react"
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
import { rpc } from "../../utils/rpc"

export const Account = () => {
  const account = useAccount()
  const { data: basicState } = rpc.general.getBasicState.useQuery()

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
      <div className="flex flex-row space-x-2">
        <Link href="/send">
          <Button>
            <UploadIcon className="h-4 w-4 mr-2" />
            Send
          </Button>
        </Link>
        <Link href="/receive">
          <Button>
            <DownloadIcon className="h-4 w-4 mr-2" /> Receive
          </Button>
        </Link>
      </div>
    </div>
  )
}
