import type { RegistrationStatus } from "../../../../backend/registration-client/computed/registration-status"
import { RegistrationStatusBadge } from "./registration-status-badge"

interface RegistrationAdvancedSettingsProperties {
  status: RegistrationStatus
}

export const RegistrationAdvancedSettings = ({
  status,
}: RegistrationAdvancedSettingsProperties) => {
  return (
    <div>
      <ul>
        {status.nodes.map(({ hostname, nodeId, registered }) => (
          <li key={nodeId} className="p-2 flex gap-2">
            {hostname ?? nodeId}
            <RegistrationStatusBadge registered={registered} size="sm" />
          </li>
        ))}
      </ul>
    </div>
  )
}
