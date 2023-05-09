import { extend } from "../../utils/class-helper"

const DialogRoot = extend(
  "DialogRoot",
  "div",
  "bg-white rounded-xl flex flex-col h-full max-w-2xl p-4 gap-4 items-start justify-start md:h-auto md:shadow-md md:px-16 md:py-8 md:gap-6"
)

const DialogTitlebar = extend(
  "DialogTitlebar",
  "div",
  "w-full flex items-center"
)

const DialogTitle = extend(
  "DialogTitle",
  "h1",
  "flex-grow flex-shrink-0 basis-auto font-bold text-lg md:text-xl"
)

const DialogTitleActionButton = extend(
  "DialogTitleActionButton",
  "button",
  "text-slate-500 hover:bg-blue-700 hover:text-white focus:ring-4 focus:outline-none focus:ring-blue-300 font-medium rounded-full p-2.5 mr-2 text-xl text-center inline-flex items-center dark:text-blue-500 dark:hover:text-white dark:focus:ring-blue-800 -md:ml-12"
)

const Dialog = {
  Root: DialogRoot,
  Titlebar: DialogTitlebar,
  Title: DialogTitle,
  TitleActionButton: DialogTitleActionButton,
}

export default Dialog
