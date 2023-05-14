import { useCallback, useState } from "react"

import { Button } from "../../../components/ui/button"
import { CardContent, CardFooter } from "../../../components/ui/card"
import { Input } from "../../../components/ui/input"

interface SubpageRecoverProperties {
  onConfirm: (mnemonic: string) => void
  onBack: () => void
}

export const SubpageRecover = ({
  onConfirm,
  onBack,
}: SubpageRecoverProperties) => {
  const [mnemonic, setMnemonic] = useState("")

  const onClickSubmit = useCallback(() => {
    onConfirm(mnemonic)
  }, [onConfirm, mnemonic])

  return (
    <>
      <CardContent>
        <p>
          Please enter the passphrase corresponding to the wallet you would like
          to load.
        </p>
        <div className="mt-6">
          <label
            htmlFor="email"
            className="font-medium text-sm mb-2 text-gray-900 block dark:text-gray-300"
          >
            Passphrase
          </label>
          <Input
            type="text"
            id="word"
            placeholder="Enter your passphrase"
            value={mnemonic}
            onChange={(event) => setMnemonic(event.target.value)}
            required
          />
        </div>
      </CardContent>
      <CardFooter className="justify-between">
        <Button variant="ghost" onClick={onBack}>
          Back
        </Button>
        <Button onClick={onClickSubmit}>Open Wallet</Button>
      </CardFooter>
    </>
  )
}
