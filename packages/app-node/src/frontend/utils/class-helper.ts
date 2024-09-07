import { cva } from "class-variance-authority"
import { type ClassValue, clsx } from "clsx"
import {
  type ComponentProps,
  type ComponentType,
  type ElementRef,
  type ForwardRefExoticComponent,
  type JSX,
  type PropsWithoutRef,
  type RefAttributes,
  createElement,
  forwardRef,
} from "react"
import { twMerge } from "tailwind-merge"

export function combine(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

export const combineVariants: typeof cva = (...inputs) => {
  const variants = cva(...inputs)
  return (properties) => combine(variants(properties))
}

/**
 * Extend a component with addtional Tailwind classes.
 *
 * Creates a new component which renders the base component with the addtional classes added to it.
 *
 * @example
 *
 * const BlueButton = extend("BlueButton", "button", "bg-blue-500 text-white")
 *
 * @see {@link https://daily.dev/blog/my-tailwind-css-utility-function-for-creating-reusable-react-components-typescript-support|Original blog post by Ido Shamun}
 *
 * @param displayName - A name for the component which can be used for debugging purposes
 * @param type - The base component, either a string for a DOM element or a React component
 * @param additionalClassNames - Additional class names to be added to the component
 */
export function extend<TElement extends keyof JSX.IntrinsicElements>(
  displayName: string,
  type: TElement,
  ...additionalClassNames: ClassValue[]
): ForwardRefExoticComponent<
  PropsWithoutRef<ComponentProps<TElement>> &
    RefAttributes<ElementRef<TElement>>
>

export function extend<
  TComponent,
  TProperties extends { className?: string | undefined },
>(
  displayName: string,
  type: TComponent & ComponentType<TProperties>,
  ...additionalClassNames: ClassValue[]
): ForwardRefExoticComponent<TProperties>

export function extend<
  TElement extends
    | ComponentType<{ className?: string | undefined }>
    | keyof JSX.IntrinsicElements,
  TProperties extends ComponentProps<TElement> = ComponentProps<TElement>,
>(
  displayName: string,
  type: TElement,
  ...additionalClassNames: ClassValue[]
): ForwardRefExoticComponent<
  PropsWithoutRef<TProperties> & RefAttributes<ElementRef<TElement>>
> {
  const Component = forwardRef<ElementRef<TElement>, TProperties>(
    function Classed({ className, ...properties }, reference) {
      return createElement(type, {
        ...properties,
        ref: reference,
        className: combine(...additionalClassNames, className),
      })
    },
  )

  Component.displayName = displayName

  return Component
}
