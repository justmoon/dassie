import * as LabelPrimitive from "@radix-ui/react-label"
import { type VariantProps, cva } from "class-variance-authority"
import { ComponentPropsWithoutRef, ElementRef, forwardRef } from "react"

import { combine } from "../../utils/class-helper"

const labelVariants = cva(
  "text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70",
)

const Label = forwardRef<
  ElementRef<typeof LabelPrimitive.Root>,
  ComponentPropsWithoutRef<typeof LabelPrimitive.Root> &
    VariantProps<typeof labelVariants>
>(({ className, ...properties }, reference) => (
  <LabelPrimitive.Root
    ref={reference}
    className={combine(labelVariants(), className)}
    {...properties}
  />
))
Label.displayName = LabelPrimitive.Root.displayName

export { Label }
