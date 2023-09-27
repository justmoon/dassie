import { useCallback } from "react"

import { Button } from "../../../components/ui/button"
import { CardContent, CardFooter } from "../../../components/ui/card"

interface SubpageGenerateProperties {
  mnemonic: string
  onConfirm: (mnemonic: string) => void
  onBack: () => void
}

export const SubpageGenerate = ({
  mnemonic,
  onConfirm,
  onBack,
}: SubpageGenerateProperties) => {
  return (
    <>
      <CardContent>
        <p>
          Here is your wallet passphrase. Keep a copy of this in a safe place.
        </p>
        <p>
          Anyone with this passphrase controls this wallet. If you lose this
          passphrase you lose your wallet. There is no recovery.
        </p>
        <div
          className="-mx-4 py-4 md:rounded-2xl md:p-4"
          // style={{
          //   background:
          //     "repeating-linear-gradient(45deg, black, black 10px, yellow 10px, yellow 20px)",
          // }}
        >
          <p className="bg-destructive font-bold shadow-inner text-destructive-foreground text-lg text-center p-4 leading-8 md:rounded-lg md:text-xl md:p-6 md:leading-9 lg:p-8 lg:leading-10">
            {mnemonic}
          </p>
        </div>
      </CardContent>
      <CardFooter className="justify-between">
        <Button variant="ghost" onClick={onBack}>
          Back
        </Button>
        <Button
          className="self-center"
          onClick={useCallback(() => {
            onConfirm(mnemonic)
          }, [onConfirm, mnemonic])}
        >
          Ok, I have backed up my passphrase
        </Button>
      </CardFooter>
    </>
  )
}
