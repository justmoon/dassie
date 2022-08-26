// Based on Ido Shamun
// https://daily.dev/blog/my-tailwind-css-utility-function-for-creating-reusable-react-components-typescript-support
import cx, { Argument } from "classnames"
import {
  Attributes,
  ClassAttributes,
  ElementType,
  FunctionComponentElement,
  ReactElement,
  createElement,
} from "react"

function classed<P extends Record<string, unknown>>(
  type: ElementType,
  ...className: Argument[]
): (properties?: (Attributes & P) | null) => FunctionComponentElement<P>

function classed<
  T extends keyof JSX.IntrinsicElements,
  P extends JSX.IntrinsicElements[T]
>(
  type: keyof JSX.IntrinsicElements,
  ...className: Argument[]
): (properties?: (ClassAttributes<T> & P) | null) => ReactElement<P, T>

function classed<P extends Record<string, unknown>>(
  type: ElementType | keyof JSX.IntrinsicElements,
  ...className: Argument[]
): (
  properties?: (Attributes & P & { className?: string }) | null
) => ReactElement<P> {
  return function Classed(properties) {
    return createElement(type, {
      ...properties,
      className: cx(properties?.className, ...className),
    })
  }
}

export default classed
