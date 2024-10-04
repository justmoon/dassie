import {
  CircleAlertIcon,
  CircleCheckIcon,
  CircleHelpIcon,
  type LucideIcon,
} from "lucide-react"
import type { ComponentPropsWithoutRef } from "react"

import { combine, extend } from "../../utils/class-helper"

export type ChecklistItemVariant = "valid" | "invalid" | "unknown"

export interface ChecklistItemProperties
  extends ComponentPropsWithoutRef<"li"> {
  variant: ChecklistItemVariant
}

const ICONS: Record<ChecklistItemVariant, LucideIcon> = {
  valid: CircleCheckIcon,
  invalid: CircleAlertIcon,
  unknown: CircleHelpIcon,
}

export function ChecklistItem({
  variant,
  children,
  className,
  ...remainingProperties
}: ChecklistItemProperties) {
  const Icon = ICONS[variant]

  return (
    <li
      className={combine("flex flex-row gap-2 items-center", className)}
      {...remainingProperties}
    >
      <Icon
        className={combine(
          "size-4",
          {
            valid: "text-green-500",
            invalid: "text-red-500",
            unknown: "text-slate-400",
          }[variant],
        )}
      />
      {children}
    </li>
  )
}

export const Checklist = extend("Checklist", "ul", "flex flex-col gap-3 py-4")
