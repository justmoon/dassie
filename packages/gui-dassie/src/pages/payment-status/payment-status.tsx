export interface PaymentStatusProperties {
  params: {
    paymentId: string
  }
}

export const PaymentStatus = ({
  params: { paymentId },
}: PaymentStatusProperties) => {
  return <div>{paymentId}</div>
}
