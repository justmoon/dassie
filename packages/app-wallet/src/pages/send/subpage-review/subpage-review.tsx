import Button from "../../../components/button/button"

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
      <div>Amount: {String(amount)}</div>
      <Button onClick={onSubmit}>Confirm</Button>
    </div>
  )
}
