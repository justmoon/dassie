import classed from "../../utils/classed"

const Button = classed(
  "button",
  "text-white focus:ring-4 focus:ring-blue-300 font-medium rounded-lg text-sm px-5 py-2.5 focus:outline-none dark:focus:ring-blue-800 inline-flex items-center justify-around",
  "enabled:bg-blue-700 enabled:hover:bg-blue-800 enabled:dark:bg-blue-600 enabled:dark:hover:bg-blue-700",
  "disabled:bg-blue-400 disabled:dark:bg-blue-500 disabled:cursor-not-allowed"
)

export default Button
