import { useCallback, useState } from "react"

import { Button } from "../../../components/button/button"

interface SubpageRecoverProperties {
  onConfirm: (mnemonic: string) => void
}

export const SubpageRecover = ({ onConfirm }: SubpageRecoverProperties) => {
  const [mnemonic, setMnemonic] = useState("")

  const onClickSubmit = useCallback(() => {
    onConfirm(mnemonic)
  }, [onConfirm, mnemonic])

  return (
    <>
      <p>
        Please enter the passphrase corresponding to the wallet you would like
        to load.
      </p>
      <div className="mb-6">
        <label
          htmlFor="email"
          className="font-medium text-sm mb-2 text-gray-900 block dark:text-gray-300"
        >
          Passphrase
        </label>
        <input
          type="text"
          id="word"
          className="border rounded-lg bg-gray-50 border-gray-300 text-sm w-full p-2.5 text-gray-900 block dark:bg-gray-700 dark:border-gray-600 dark:text-white dark:placeholder-gray-400 focus:border-blue-500 focus:ring-blue-500 dark:focus:border-blue-500 dark:focus:ring-blue-500"
          placeholder="Enter your passphrase"
          value={mnemonic}
          onChange={(event) => setMnemonic(event.target.value)}
          required
        />
      </div>
      <div className="flex flex-col gap-4">
        <Button onClick={onClickSubmit}>Open Wallet</Button>
      </div>
    </>
  )
}
