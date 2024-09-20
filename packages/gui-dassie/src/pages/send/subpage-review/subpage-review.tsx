import { Amount } from "../../../components/amount/amount"
import { Button } from "../../../components/ui/button"
import { CardContent, CardFooter } from "../../../components/ui/card"
import { USD_SPECIFICATION } from "../../../constants/currency"

interface SubpageReviewProperties {
  paymentPointer: string
  amount: bigint
  onSubmit: () => void
  onBack: () => void
}
export const SubpageReview = ({
  paymentPointer,
  amount,
  onSubmit,
  onBack,
}: SubpageReviewProperties) => {
  return (
    <>
      <CardContent>
        <div>Payment Pointer: {paymentPointer}</div>
        <div>
          Amount: <Amount value={amount} currency={USD_SPECIFICATION} />
        </div>
      </CardContent>
      <CardFooter className="justify-between space-x-2">
        <Button
          variant="ghost"
          onClick={() => {
            onBack()
          }}
        >
          Back
        </Button>
        <Button onClick={onSubmit}>Confirm</Button>
      </CardFooter>
    </>
  )
}
