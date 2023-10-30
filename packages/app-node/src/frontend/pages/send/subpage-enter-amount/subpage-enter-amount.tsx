import { useState } from "react"
import { NumericFormat } from "react-number-format"

import { Button } from "../../../components/ui/button"
import { CardContent, CardFooter } from "../../../components/ui/card"
import { USD_SPECIFICATION } from "../../../constants/currency"
import { parseDecimalToBigInt } from "../../../utils/currency"

interface SubpageEnterAmountProperties {
  paymentPointer: string
  onSubmit: (amount: bigint) => void
  onBack: () => void
}

const DEFAULT_AMOUNT = "5"

export const SubpageEnterAmount = ({
  paymentPointer,
  onSubmit,
  onBack,
}: SubpageEnterAmountProperties) => {
  const [amount, setAmount] = useState(DEFAULT_AMOUNT)

  const prefix = `${USD_SPECIFICATION.symbol}\u2009`

  return (
    <>
      <CardContent>
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
      </CardContent>
      <CardFooter className="justify-between space-x-2">
        <Button variant="ghost" onClick={() => onBack()}>
          Back
        </Button>
        <Button
          onClick={() =>
            onSubmit(
              parseDecimalToBigInt(amount, USD_SPECIFICATION.totalPrecision),
            )
          }
        >
          Review
        </Button>
      </CardFooter>
    </>
  )
}
