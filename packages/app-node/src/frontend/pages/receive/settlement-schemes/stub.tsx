import { useLocation } from "wouter"

import { Button } from "../../../components/ui/button"
import { rpc } from "../../../utils/rpc"

export default function StubDeposit() {
  const stubDepositMutation = rpc.settlement.stubDeposit.useMutation()
  const [, setLocation] = useLocation()

  return (
    <div>
      <p>
        The &quot;stub&quot; settlement method only simulates a ledger rather
        than using a real one. You can click the button below to simulate
        receiving funds on the underlying ledger.
      </p>
      <Button
        className="mt-4"
        onClick={() => {
          stubDepositMutation.mutate("100000000000")
          setLocation("/")
        }}
      >
        Receive $100
      </Button>
    </div>
  )
}
