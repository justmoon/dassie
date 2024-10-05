import { ChevronDownIcon } from "lucide-react"
import { useState } from "react"

import { useRemoteSignal } from "@dassie/lib-reactive-rpc/client"

import { Button } from "../../../../components/ui/button"
import { combine } from "../../../../utils/class-helper"
import { rpc } from "../../../../utils/rpc"
import { RegistrationAdvancedSettings } from "./registration-advanced"
import { RegistrationStatusBadge } from "./registration-status-badge"

export const RegistrationSettings = () => {
  const [isAdvancedOpen, setAdvancedOpen] = useState(false)
  const registrationStatus = useRemoteSignal(
    rpc.registrationClient.subscribeRegistrationStatus,
  )

  if (!registrationStatus) return <div>Loading...</div>

  const { registrationRatio, threshold } = registrationStatus

  const isThresholdMet = registrationRatio >= threshold

  return (
    <div>
      <div className="flex gap-3 items-center">
        <RegistrationStatusBadge registered={isThresholdMet} />
        <Button
          variant="default"
          size="sm"
          onClick={() => {
            setAdvancedOpen((state) => !state)
          }}
        >
          Advanced
          <ChevronDownIcon
            className={combine(
              "size-4 transition-transform",
              isAdvancedOpen && "-rotate-180",
            )}
          />
        </Button>
      </div>
      <div className={combine("hidden", isAdvancedOpen && "block")}>
        <RegistrationAdvancedSettings status={registrationStatus} />
      </div>
    </div>
  )
}
