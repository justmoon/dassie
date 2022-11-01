import { nanoid } from "nanoid"
import { ArrowLeft } from "phosphor-react"
import { useState } from "react"
import { Link, useLocation } from "wouter"

import Dialog from "../../components/dialog/dialog"
import { trpc } from "../../utils/trpc"
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
  enterPaymentPointer: "Send to",
  enterAmount: "Choose Amount",
  review: "Review",
} as const

export const Send = () => {
  const [subpage, setSubpage] = useState<SubpageState>({
    subpage: "enterPaymentPointer",
    paymentId: nanoid(),
  })
  const [, setLocation] = useLocation()

  const createPayment = trpc.createPayment.useMutation()

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
      : undefined

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
        }
      )
    }
  }

  const subpageElement = (() => {
    switch (subpage.subpage) {
      case "enterPaymentPointer": {
        return <SubpageEnterPaymentPointer onSubmit={onSubmitPaymentPointer} />
      }
      case "enterAmount": {
        return (
          <SubpageEnterAmount
            paymentPointer={subpage.paymentPointer}
            onSubmit={onSubmitAmount}
          />
        )
      }
      case "review": {
        return (
          <SubpageReview
            paymentPointer={subpage.paymentPointer}
            amount={subpage.amount}
            onSubmit={onConfirmSend}
          />
        )
      }
    }
  })()

  return (
    <div className="flex h-full items-center justify-center">
      <Dialog.Root>
        <Dialog.Titlebar>
          {subpage.subpage === "enterPaymentPointer" ? (
            <Link href="/">
              <Dialog.TitleActionButton>
                <ArrowLeft />
                <span className="sr-only">Go back</span>
              </Dialog.TitleActionButton>
            </Link>
          ) : onBack ? (
            <Dialog.TitleActionButton onClick={onBack}>
              <ArrowLeft />
              <span className="sr-only">Go back</span>
            </Dialog.TitleActionButton>
          ) : undefined}
          <h1 className="flex-grow flex-shrink-0 basis-auto font-bold text-lg md:text-xl">
            {SUBPAGE_TITLES[subpage.subpage]}
          </h1>
        </Dialog.Titlebar>
        {subpageElement}
      </Dialog.Root>
    </div>
  )
}
