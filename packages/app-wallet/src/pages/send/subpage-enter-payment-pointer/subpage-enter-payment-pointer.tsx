import { useState } from "react"

import Button from "../../../components/button/button"
import { trpc } from "../../../utils/trpc"

interface SubpageEnterPaymentPointerProperties {
  onSubmit: (paymentPointer: string) => void
}

export const SubpageEnterPaymentPointer = ({
  onSubmit,
}: SubpageEnterPaymentPointerProperties) => {
  const [paymentPointer, setPaymentPointer] = useState("$n2.localhost:4001")
  const paymentPointerInfo = trpc.resolvePaymentPointer.useQuery({
    paymentPointer,
  })

  return (
    <div>
      <div className="mb-6">
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
        {paymentPointerInfo.status === "success" && (
          <Button onClick={() => onSubmit(paymentPointer)}>Continue</Button>
        )}
      </div>
    </div>
  )
}
