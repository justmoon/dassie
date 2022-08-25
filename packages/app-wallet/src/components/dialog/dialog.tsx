import cx from "classnames"
import type {
  ButtonHTMLAttributes,
  DetailedHTMLProps,
  ForwardedRef,
  HTMLAttributes,
} from "react"
import { forwardRef } from "react"

const DialogRoot = (
  {
    children,
    className,
    ...other
  }: DetailedHTMLProps<HTMLAttributes<HTMLDivElement>, HTMLDivElement>,
  reference: ForwardedRef<HTMLDivElement>
) => {
  return (
    <div
      ref={reference}
      className={cx(
        "bg-white rounded-xl flex flex-col h-full max-w-2xl p-4 gap-4 items-start justify-start md:h-auto md:shadow-md md:px-16 md:py-8 md:gap-6 ",
        className
      )}
      {...other}
    >
      {children}
    </div>
  )
}

const DialogTitlebar = (
  {
    children,
    className,
    ...other
  }: DetailedHTMLProps<HTMLAttributes<HTMLDivElement>, HTMLDivElement>,
  reference: ForwardedRef<HTMLDivElement>
) => {
  return (
    <div
      ref={reference}
      className={cx("w-full flex items-center", className)}
      {...other}
    >
      {children}
    </div>
  )
}

const DialogTitle = (
  {
    children,
    className,
    ...other
  }: DetailedHTMLProps<HTMLAttributes<HTMLHeadingElement>, HTMLHeadingElement>,
  reference: ForwardedRef<HTMLHeadingElement>
) => {
  return (
    <h1
      ref={reference}
      className={cx(
        "flex-grow flex-shrink-0 basis-auto font-bold text-lg md:text-xl",
        className
      )}
      {...other}
    >
      {children}
    </h1>
  )
}

const DialogTitleActionButton = (
  {
    children,
    className,
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
        "text-slate-500 hover:bg-blue-700 hover:text-white focus:ring-4 focus:outline-none focus:ring-blue-300 font-medium rounded-full p-2.5 mr-2 text-xl text-center inline-flex items-center dark:text-blue-500 dark:hover:text-white dark:focus:ring-blue-800 -md:ml-12",
        className
      )}
      {...other}
    >
      {children}
    </button>
  )
}

const Dialog = {
  Root: forwardRef(DialogRoot),
  Titlebar: forwardRef(DialogTitlebar),
  Title: forwardRef(DialogTitle),
  TitleActionButton: forwardRef(DialogTitleActionButton),
}

export default Dialog
