import { useState } from "react"

import { Button } from "../../../components/ui/button"
import { CardContent, CardFooter } from "../../../components/ui/card"
import { trpc } from "../../../utils/trpc"

interface SubpageEnterPaymentPointerProperties {
  onSubmit: (paymentPointer: string) => void
  onBack: () => void
}

export const SubpageEnterPaymentPointer = ({
  onSubmit,
  onBack,
}: SubpageEnterPaymentPointerProperties) => {
  const [paymentPointer, setPaymentPointer] = useState("$n2.localhost")
  const paymentPointerInfo = trpc.resolvePaymentPointer.useQuery({
    paymentPointer,
  })

  return (
    <>
      <CardContent>
        <label
          htmlFor="payment_pointer"
          className="block mb-2 text-sm font-medium text-gray-900 dark:text-gray-300"
        >
          Payment Pointer
        </label>
        <input
          type="text"
          id="payment_pointer"
          className="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5 dark:bg-gray-700 dark:border-gray-600 dark:placeholder-gray-400 dark:text-white dark:focus:ring-blue-500 dark:focus:border-blue-500"
          placeholder="$example.com"
          required
          value={paymentPointer}
          onChange={(event) => setPaymentPointer(event.target.value)}
        />
        {paymentPointerInfo.status === "loading" && <div>Loading...</div>}
        {paymentPointerInfo.status === "error" && (
          <div>Error: {paymentPointerInfo.error.message}</div>
        )}
      </CardContent>
      <CardFooter className="justify-between space-x-2">
        <Button variant="ghost" onClick={() => onBack()}>
          Cancel
        </Button>
        {paymentPointerInfo.status === "success" && (
          <Button onClick={() => onSubmit(paymentPointer)}>Continue</Button>
        )}
      </CardFooter>
    </>
  )
}
