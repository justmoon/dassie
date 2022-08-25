import { useCallback } from "react"

import Button from "../../../components/button/button"

interface SubpageGenerateProperties {
  mnemonic: string
  onConfirm: (mnemonic: string) => void
}

export const SubpageGenerate = ({
  mnemonic,
  onConfirm,
}: SubpageGenerateProperties) => {
  return (
    <>
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
        <p className="bg-slate-100 font-bold shadow-inner text-blue-900 text-lg text-center p-4 leading-8 md:rounded-lg md:text-xl md:p-6 md:leading-9 lg:p-8 lg:leading-10">
          {mnemonic}
        </p>
      </div>
      <Button
        className="self-center"
        onClick={useCallback(() => {
          onConfirm(mnemonic)
        }, [onConfirm, mnemonic])}
      >
        Ok, I have backed up my passphrase
      </Button>
    </>
  )
}
