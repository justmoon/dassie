/* @refresh reload */
import "@unocss/reset/tailwind.css"
import { createRoot } from "react-dom/client"
import "uno.css"
import "virtual:unocss-devtools"

import { assertDefined } from "@xen-ilp/lib-type-utils"

import App from "./app"

const rootElement = document.querySelector("#root")
assertDefined(rootElement)
const root = createRoot(rootElement)
root.render(<App />)
