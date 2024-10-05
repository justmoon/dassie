import type { ComponentPropsWithoutRef } from "react"

import { combine } from "../../../utils/class-helper"
import { RegistrationSettings } from "./registration/registration"

export function OwnNodeDetail({
  className,
  ...remainingProperties
}: ComponentPropsWithoutRef<"div">) {
  return (
    <div
      className={combine("flex flex-col gap-4", className)}
      {...remainingProperties}
    >
      <h2 className="text-3xl font-bold">Details for Your Node</h2>
      <h4 className="text-xl">Tier 1 Registration</h4>
      <RegistrationSettings />
    </div>
  )
}
