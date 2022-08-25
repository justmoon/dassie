import cx from "classnames"
import type {
  ButtonHTMLAttributes,
  DetailedHTMLProps,
  ForwardedRef,
} from "react"
import { forwardRef } from "react"

const Button = (
  {
    children,
    className,
    disabled,
    ...other
  }: DetailedHTMLProps<
    ButtonHTMLAttributes<HTMLButtonElement>,
    HTMLButtonElement
  >,
  reference: ForwardedRef<HTMLButtonElement>
) => {
  return (
    <button
      ref={reference}
      type="button"
      className={cx(
        "text-white focus:ring-4 focus:ring-blue-300 font-medium rounded-lg text-sm px-5 py-2.5 focus:outline-none dark:focus:ring-blue-800",
        {
          "bg-blue-700 hover:bg-blue-800 dark:bg-blue-600 dark:hover:bg-blue-700":
            !disabled,
          "bg-blue-400 dark:bg-blue-500 cursor-not-allowed": disabled,
        },
        className
      )}
      disabled={disabled}
      {...other}
    >
      {children}
    </button>
  )
}

export default forwardRef(Button)
