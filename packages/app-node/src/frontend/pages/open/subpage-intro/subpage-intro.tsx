import { Button } from "../../../components/ui/button"
import { CardContent, CardFooter } from "../../../components/ui/card"

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
      <CardContent>
        <p>
          Welcome to Dassie Wallet. The first step is to create or recover a
          wallet.
        </p>
      </CardContent>
      <CardFooter>
        <div className="flex flex-col gap-4 self-center items-center">
          <Button onClick={onGenerateClick}>Generate a new wallet</Button>
          <button
            className="font-medium text-sm text-blue-600 dark:text-blue-500 hover:underline"
            onClick={onRecoverClick}
          >
            Recover an existing wallet
          </button>
        </div>
      </CardFooter>
    </>
  )
}
