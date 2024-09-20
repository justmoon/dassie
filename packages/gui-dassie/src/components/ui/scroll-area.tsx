import * as ScrollAreaPrimitive from "@radix-ui/react-scroll-area"
import {
  type ComponentPropsWithoutRef,
  type ElementRef,
  forwardRef,
} from "react"

import { combine } from "../../utils/class-helper"

const ScrollArea = forwardRef<
  ElementRef<typeof ScrollAreaPrimitive.Root>,
  ComponentPropsWithoutRef<typeof ScrollAreaPrimitive.Root>
>(({ className, children, ...properties }, reference) => (
  <ScrollAreaPrimitive.Root
    ref={reference}
    className={combine("relative overflow-hidden", className)}
    {...properties}
  >
    <ScrollAreaPrimitive.Viewport className="h-full w-full rounded-[inherit]">
      {children}
    </ScrollAreaPrimitive.Viewport>
    <ScrollBar />
    <ScrollAreaPrimitive.Corner />
  </ScrollAreaPrimitive.Root>
))
ScrollArea.displayName = ScrollAreaPrimitive.Root.displayName

const ScrollBar = forwardRef<
  ElementRef<typeof ScrollAreaPrimitive.ScrollAreaScrollbar>,
  ComponentPropsWithoutRef<typeof ScrollAreaPrimitive.ScrollAreaScrollbar>
>(({ className, orientation = "vertical", ...properties }, reference) => (
  <ScrollAreaPrimitive.ScrollAreaScrollbar
    ref={reference}
    orientation={orientation}
    className={combine(
      "flex touch-none select-none transition-colors",
      orientation === "vertical" &&
        "h-full w-2.5 border-l border-l-transparent p-[1px]",
      orientation === "horizontal" &&
        "h-2.5 flex-col border-t border-t-transparent p-[1px]",
      className,
    )}
    {...properties}
  >
    <ScrollAreaPrimitive.ScrollAreaThumb className="relative flex-1 rounded-full bg-border" />
  </ScrollAreaPrimitive.ScrollAreaScrollbar>
))
ScrollBar.displayName = ScrollAreaPrimitive.ScrollAreaScrollbar.displayName

export { ScrollArea, ScrollBar }
