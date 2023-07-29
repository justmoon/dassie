import { createRoot } from "react-dom/client"
import "virtual:uno.css"

import { assertDefined } from "@dassie/lib-type-utils"
import "@dassie/meta-unocss-config/base.css"

import Root from "./root"

const rootElement = document.querySelector("#root")
assertDefined(rootElement)
const root = createRoot(rootElement)
root.render(<Root />)
