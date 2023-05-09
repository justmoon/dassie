import { type ClassValue, clsx } from "clsx"
import {
  ComponentProps,
  ComponentType,
  ElementRef,
  ForwardRefExoticComponent,
  type JSX,
  PropsWithoutRef,
  RefAttributes,
  createElement,
  forwardRef,
} from "react"
import { twMerge } from "tailwind-merge"

export function combine(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
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
 * @param addtionalClassNames - Additional class names to be added to the component
 */
export function extend<TElement extends keyof JSX.IntrinsicElements>(
  displayName: string,
  type: TElement,
  ...addtionalClassNames: ClassValue[]
): ForwardRefExoticComponent<
  PropsWithoutRef<ComponentProps<TElement>> &
    RefAttributes<ElementRef<TElement>>
>

export function extend<
  TComponent,
  TProperties extends { className?: string | undefined }
>(
  displayName: string,
  type: TComponent & ComponentType<TProperties>,
  ...addtionalClassNames: ClassValue[]
): ForwardRefExoticComponent<TProperties>

export function extend<
  TElement extends
    | ComponentType<{ className?: string | undefined }>
    | keyof JSX.IntrinsicElements
>(
  displayName: string,
  type: TElement,
  ...addtionalClassNames: ClassValue[]
): ForwardRefExoticComponent<
  PropsWithoutRef<ComponentProps<TElement>> & RefAttributes<unknown>
> {
  const Component = forwardRef(function Classed(
    { className, ...properties }: ComponentProps<TElement>,
    reference
  ) {
    return createElement(type, {
      ...properties,
      ref: reference,
      className: combine(className, ...addtionalClassNames),
    })
  })

  Component.displayName = displayName

  return Component
}
