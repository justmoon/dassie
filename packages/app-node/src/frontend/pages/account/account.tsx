import { Link } from "wouter"

import { Amount } from "../../components/amount/amount"
import { Button } from "../../components/button/button"
import { useAccount } from "../../hooks/use-account"

export const Account = () => {
  const account = useAccount()

  return (
    <div className="flex flex-col items-center">
      <div className="flex bg-slate-200 rounded-xl m-4 p-4 text-dark text-center max-w-md w-full justify-center text-4xl md:text-5xl">
        <Amount balance={account.balance} currency={account.currency} />
      </div>
      <Link href="/send">
        <Button variant="link">Send</Button>
      </Link>
    </div>
  )
}
