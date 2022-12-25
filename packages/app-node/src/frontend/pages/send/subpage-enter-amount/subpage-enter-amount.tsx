import { useState } from "react"
import { NumericFormat } from "react-number-format"

import Button from "../../../components/button/button"
import { USD_SPECIFICATION } from "../../../constants/currency"
import { parseDecimalToBigInt } from "../../../utils/currency"

interface SubpageEnterAmountProperties {
  paymentPointer: string
  onSubmit: (amount: bigint) => void
}

export const SubpageEnterAmount = ({
  paymentPointer,
  onSubmit,
}: SubpageEnterAmountProperties) => {
  const [amount, setAmount] = useState("40")

  const prefix = `${USD_SPECIFICATION.symbol}\u2009`

  return (
    <div>
      <div className="mb-6">
        <div>To: {paymentPointer}</div>
        <label
          htmlFor="payment_pointer"
          className="block mb-2 text-sm font-medium text-gray-900 dark:text-gray-300"
        >
          Amount
        </label>
        <NumericFormat
          type="text"
          id="payment_pointer"
          className="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5 dark:bg-gray-700 dark:border-gray-600 dark:placeholder-gray-400 dark:text-white dark:focus:ring-blue-500 dark:focus:border-blue-500"
          placeholder={`${prefix}123.45`}
          allowNegative={false}
          prefix={prefix}
          required
          value={amount}
          valueIsNumericString={true}
          thousandsGroupStyle="thousand"
          thousandSeparator=","
          onValueChange={({ value }) => setAmount(value)}
        />
        <Button
          onClick={() =>
            onSubmit(
              parseDecimalToBigInt(amount, USD_SPECIFICATION.totalPrecision)
            )
          }
        >
          Review
        </Button>
      </div>
    </div>
  )
}
