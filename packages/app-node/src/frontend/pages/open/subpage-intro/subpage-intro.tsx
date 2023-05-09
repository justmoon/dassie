import { Button } from "../../../components/ui/button"

interface SubpageIntroProperties {
  onGenerateClick: () => void
  onRecoverClick: () => void
}

export const SubpageIntro = ({
  onGenerateClick,
  onRecoverClick,
}: SubpageIntroProperties) => {
  return (
    <>
      <p>
        Welcome to Dassie Wallet. The first step is to create or recover a
        wallet.
      </p>
      <div className="flex flex-col gap-4 self-center items-center">
        <Button onClick={onGenerateClick}>Generate a new wallet</Button>
        <button
          className="font-medium text-sm text-blue-600 dark:text-blue-500 hover:underline"
          onClick={onRecoverClick}
        >
          Recover an existing wallet
        </button>
      </div>
    </>
  )
}
