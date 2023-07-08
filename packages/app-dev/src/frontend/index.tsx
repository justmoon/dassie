/* @refresh reload */
import "@unocss/reset/tailwind.css"
import { createRoot } from "react-dom/client"
import "virtual:uno.css"
import "virtual:unocss-devtools"

import { assertDefined } from "@dassie/lib-type-utils"

import Root from "./root"

const rootElement = document.querySelector("#root")
assertDefined(rootElement)
const root = createRoot(rootElement)
root.render(<Root />)
