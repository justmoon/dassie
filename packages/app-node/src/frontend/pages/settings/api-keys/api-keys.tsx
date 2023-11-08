import { CheckIcon, CopyIcon, EyeIcon, EyeOffIcon, XIcon } from "lucide-react"
import { useRef, useState } from "react"

import { Button } from "../../../components/ui/button"
import { trpc } from "../../../utils/trpc"

interface BtpTokenAttributes {
  token: string
}

const COPY_DONE_TIMEOUT = 1000

/**
 * Shows the last four characters of the string and replaces the reset with the bullet character.
 */
const hideMostOfString = (subject: string) => {
  const length = subject.length
  const hiddenCharacters = "•".repeat(length - 4)
  const lastFourCharacters = subject.slice(length - 4, length)
  return `${hiddenCharacters}${lastFourCharacters}`
}

const BtpToken = ({ token }: BtpTokenAttributes) => {
  const [isShown, setIsShown] = useState(false)
  const [isCopied, setIsCopied] = useState(false)
  const timerReference = useRef<NodeJS.Timeout | null>(null)

  const basicState = trpc.general.getBasicState.useQuery()
  const currentKeysQuery = trpc.apiKeys.getCurrentKeys.useQuery()
  const deleteTokenMutation = trpc.apiKeys.removeBtpToken.useMutation({
    onSuccess: async () => {
      await currentKeysQuery.refetch()
    },
  })

  const btpUrl = `btp+wss://:${token}@${
    basicState.data?.hostname ?? "localhost"
  }/btp`

  return (
    <div className="flex flex-row space-x-2 items-center">
      <div>BTP Token</div>
      <div className="flex flex-row items-center font-mono rounded-md border p-2 space-x-2">
        <div>{isShown ? token : hideMostOfString(token)}</div>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => setIsShown((state) => !state)}
        >
          {isShown ? (
            <EyeIcon className="w-4 h-4" />
          ) : (
            <EyeOffIcon className="w-4 h-4" />
          )}
        </Button>
        {navigator.clipboard && (
          <div>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                navigator.clipboard
                  .writeText(btpUrl)
                  .then(() => {
                    if (timerReference.current) {
                      clearTimeout(timerReference.current)
                    }

                    timerReference.current = setTimeout(() => {
                      setIsCopied(false)
                    }, COPY_DONE_TIMEOUT)

                    setIsCopied(true)
                  })
                  .catch(() => {})
              }}
            >
              {isCopied ? (
                <CheckIcon className="w-4 h-4" />
              ) : (
                <CopyIcon className="w-4 h-4" />
              )}
            </Button>
          </div>
        )}
      </div>
      <div>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => deleteTokenMutation.mutate(token)}
        >
          <XIcon className="w-4 h-4 text-red" />
        </Button>
      </div>
    </div>
  )
}

export const ApiKeysSettings = () => {
  const currentKeysQuery = trpc.apiKeys.getCurrentKeys.useQuery()
  const addBtpToken = trpc.apiKeys.addBtpToken.useMutation({
    onSuccess: async () => {
      await currentKeysQuery.refetch()
    },
  })

  if (currentKeysQuery.isLoading) {
    return <div>Loading...</div>
  }

  if (currentKeysQuery.error) {
    return <div>Error: {currentKeysQuery.error.message}</div>
  }

  return (
    <div>
      {currentKeysQuery.data.length === 0 ? (
        <div className="pb-4">No API keys created yet. Add one below.</div>
      ) : (
        <div className="flex flex-col space-y-2 pb-4">
          {currentKeysQuery.data.map((key) => (
            <BtpToken key={key} token={key} />
          ))}
        </div>
      )}
      <Button
        onClick={() => addBtpToken.mutate()}
        disabled={addBtpToken.isLoading}
      >
        Create BTP Token
      </Button>
    </div>
  )
}
