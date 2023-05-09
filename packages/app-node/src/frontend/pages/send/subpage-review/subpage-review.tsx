import { Amount } from "../../../components/amount/amount"
import { Button } from "../../../components/ui/button"
import { USD_SPECIFICATION } from "../../../constants/currency"

interface SubpageReviewProperties {
  paymentPointer: string
  amount: bigint
  onSubmit: () => void
}
export const SubpageReview = ({
  paymentPointer,
  amount,
  onSubmit,
}: SubpageReviewProperties) => {
  return (
    <div>
      <div>Payment Pointer: {paymentPointer}</div>
      <div>
        Amount: <Amount balance={amount} currency={USD_SPECIFICATION} />
      </div>
      <Button onClick={onSubmit}>Confirm</Button>
    </div>
  )
}
