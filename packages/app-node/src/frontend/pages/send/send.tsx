import { nanoid } from "nanoid"
import { useState } from "react"
import { useLocation } from "wouter"

import { Card, CardHeader, CardTitle } from "../../components/ui/card"
import { rpc } from "../../utils/rpc"
import { SubpageEnterAmount } from "./subpage-enter-amount/subpage-enter-amount"
import { SubpageEnterPaymentPointer } from "./subpage-enter-payment-pointer/subpage-enter-payment-pointer"
import { SubpageReview } from "./subpage-review/subpage-review"

interface SubpageEnterPaymentPointerState {
  subpage: "enterPaymentPointer"
  paymentId: string
}

interface SubpageEnterAmountState {
  subpage: "enterAmount"
  paymentId: string
  paymentPointer: string
}

interface SubpageReviewState {
  subpage: "review"
  paymentId: string
  paymentPointer: string
  amount: bigint
}

type SubpageState =
  | SubpageEnterPaymentPointerState
  | SubpageEnterAmountState
  | SubpageReviewState

const SUBPAGE_TITLES: Record<SubpageState["subpage"], string> = {
  enterPaymentPointer: "Send Payment",
  enterAmount: "Choose Amount",
  review: "Review",
} as const

export const Send = () => {
  const [subpage, setSubpage] = useState<SubpageState>({
    subpage: "enterPaymentPointer",
    paymentId: nanoid(),
  })
  const [, setLocation] = useLocation()

  const createPayment = rpc.payment.createPayment.useMutation()

  const onBack =
    subpage.subpage === "enterAmount"
      ? () =>
          setSubpage({
            subpage: "enterPaymentPointer",
            paymentId: subpage.paymentId,
          })
      : subpage.subpage === "review"
        ? () =>
            setSubpage({
              subpage: "enterAmount",
              paymentId: subpage.paymentId,
              paymentPointer: subpage.paymentPointer,
            })
        : () => {
            setLocation("/")
          }

  const onSubmitPaymentPointer = (paymentPointer: string) => {
    setSubpage({
      subpage: "enterAmount",
      paymentId: subpage.paymentId,
      paymentPointer,
    })
  }

  const onSubmitAmount = (amount: bigint) => {
    if (subpage.subpage === "enterAmount") {
      setSubpage({
        subpage: "review",
        paymentId: subpage.paymentId,
        paymentPointer: subpage.paymentPointer,
        amount,
      })
    }
  }

  const onConfirmSend = () => {
    if (subpage.subpage === "review") {
      createPayment.mutate(
        {
          paymentId: subpage.paymentId,
          paymentPointer: subpage.paymentPointer,
          amount: subpage.amount.toString(),
        },
        {
          onSuccess() {
            setLocation(`/payment/${subpage.paymentId}`)
          },
        },
      )
    }
  }

  const subpageElement = (() => {
    switch (subpage.subpage) {
      case "enterPaymentPointer": {
        return (
          <SubpageEnterPaymentPointer
            onSubmit={onSubmitPaymentPointer}
            onBack={onBack}
          />
        )
      }
      case "enterAmount": {
        return (
          <SubpageEnterAmount
            paymentPointer={subpage.paymentPointer}
            onSubmit={onSubmitAmount}
            onBack={onBack}
          />
        )
      }
      case "review": {
        return (
          <SubpageReview
            paymentPointer={subpage.paymentPointer}
            amount={subpage.amount}
            onSubmit={onConfirmSend}
            onBack={onBack}
          />
        )
      }
    }
  })()

  return (
    <div className="flex h-full items-center justify-center">
      <Card>
        <CardHeader>
          <CardTitle className="flex-grow flex-shrink-0 basis-auto font-bold text-lg md:text-xl">
            {SUBPAGE_TITLES[subpage.subpage]}
          </CardTitle>
        </CardHeader>
        {subpageElement}
      </Card>
    </div>
  )
}
