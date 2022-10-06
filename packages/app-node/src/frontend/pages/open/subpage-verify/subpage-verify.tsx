import { useCallback, useMemo, useState } from "react"

import Button from "../../../components/button/button"

interface SubpageVerifyProperties {
  mnemonic: string
  onConfirm: (mnemonic: string) => void
}

export const SubpageVerify = ({
  mnemonic,
  onConfirm,
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
      <p>
        To confirm that you have saved the passphrase correctly, please enter
        word #{chosenWordIndex + 1} of the passphrase.
      </p>
      <div className="mb-6">
        <label
          htmlFor="email"
          className="font-medium text-sm mb-2 text-gray-900 block dark:text-gray-300"
        >
          Word #{chosenWordIndex + 1}
        </label>
        <input
          type="text"
          id="word"
          className="border rounded-lg bg-gray-50 border-gray-300 text-sm w-full p-2.5 text-gray-900 block dark:bg-gray-700 dark:border-gray-600 dark:text-white dark:placeholder-gray-400 focus:border-blue-500 focus:ring-blue-500 dark:focus:border-blue-500 dark:focus:ring-blue-500"
          placeholder=""
          value={enteredWord}
          onChange={(event) => setEnteredWord(event.target.value)}
          required
        />
      </div>
      <div className="flex flex-col gap-4">
        <Button disabled={!wordValid} onClick={onClickVerify}>
          Verify
        </Button>
      </div>
    </>
  )
}
