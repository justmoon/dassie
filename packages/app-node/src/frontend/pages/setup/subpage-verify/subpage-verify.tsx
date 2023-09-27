import { useCallback, useMemo, useState } from "react"

import { Button } from "../../../components/ui/button"
import { CardContent, CardFooter } from "../../../components/ui/card"
import { Input } from "../../../components/ui/input"

interface SubpageVerifyProperties {
  mnemonic: string
  onConfirm: (mnemonic: string) => void
  onBack: () => void
}

export const SubpageVerify = ({
  mnemonic,
  onConfirm,
  onBack,
}: SubpageVerifyProperties) => {
  const mnemonicArray = useMemo(() => mnemonic.split(" "), [mnemonic])
  const chosenWordIndex = useMemo(
    () => Math.floor(Math.random() * mnemonicArray.length),
    [mnemonicArray]
  )
  const [enteredWord, setEnteredWord] = useState("")

  const onClickVerify = useCallback(() => {
    onConfirm(mnemonic)
  }, [onConfirm, mnemonic])

  const wordValid =
    enteredWord === mnemonicArray[chosenWordIndex] ||
    (import.meta.env.DEV && enteredWord === "dev")

  return (
    <>
      <CardContent>
        <p>
          To confirm that you have saved the passphrase correctly, please enter
          word #{chosenWordIndex + 1} of the passphrase.
        </p>
        <div className="my-6">
          <label
            htmlFor="email"
            className="font-medium text-sm mb-2 text-gray-900 block dark:text-gray-300"
          >
            Word #{chosenWordIndex + 1}
          </label>
          <Input
            type="text"
            id="word"
            placeholder=""
            value={enteredWord}
            onChange={(event) => setEnteredWord(event.target.value)}
            required
          />
        </div>
      </CardContent>
      <CardFooter className="justify-between">
        <Button variant="ghost" onClick={onBack}>
          Back
        </Button>
        <Button disabled={!wordValid} onClick={onClickVerify}>
          Verify
        </Button>
      </CardFooter>
    </>
  )
}
