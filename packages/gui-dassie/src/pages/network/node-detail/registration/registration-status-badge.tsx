import { combine } from "../../../../utils/class-helper"

interface RegistrationStatusBadgeProperties {
  registered: boolean
  size?: "sm" | "md" | undefined
}

export const RegistrationStatusBadge = ({
  registered,
  size = "md",
}: RegistrationStatusBadgeProperties) => {
  return (
    <div
      className={combine(
        "rounded-full px-2 py-1 font-bold uppercase",
        registered ? "bg-green-700" : "bg-red-800",
        {
          sm: "text-xs",
          md: "text-sm",
        }[size],
      )}
    >
      <span className="drop-shadow">
        {registered ? "Registered" : "Not registered"}
      </span>
    </div>
  )
}
