import { LinkButton } from "../../components/button/button"
import { useAccount } from "../../hooks/use-account"

export const Account = () => {
  const account = useAccount()

  const totalPrecisionDividend = 10n ** BigInt(account.currency.totalPrecision)
  10n ** BigInt(account.currency.totalPrecision - account.currency.precision)
  const integerPart = account.balance / totalPrecisionDividend
  const fractionalPartString = (account.balance % totalPrecisionDividend)
    .toString()
    .padStart(account.currency.totalPrecision, "0")
  const emphasizedPart = fractionalPartString.slice(
    0,
    account.currency.precision
  )
  const deemphasizedPart = fractionalPartString.slice(
    account.currency.precision
  )

  return (
    <div className="flex flex-col items-center">
      <div className="flex bg-slate-200 rounded-xl m-4 p-4  text-dark text-center max-w-md w-full justify-center items-baseline">
        <div className="mr-1 text-4xl md:text-5xl">
          {account.currency.symbol}
        </div>
        <div className="text-4xl md:text-5xl">
          {integerPart.toLocaleString(undefined, { useGrouping: true })}
        </div>
        <div className="text-2xl">.</div>
        <div className="text-2xl">{emphasizedPart}</div>
        <div className="text-2xl text-gray-400 hidden md:block">
          {deemphasizedPart.slice(0, 2)}
        </div>
      </div>
      <LinkButton href="/send">Send</LinkButton>
    </div>
  )
}
