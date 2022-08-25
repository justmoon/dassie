/* @refresh reload */
import "@unocss/reset/tailwind.css"
import { createRoot } from "react-dom/client"
import "uno.css"

import { assertDefined } from "@dassie/lib-type-utils"

import App from "./app"
import "./index.css"
import Uplink from "./modules/uplink"

const rootElement = document.querySelector("#root")
assertDefined(rootElement)
const root = createRoot(rootElement)
root.render(<App />)

new Uplink()
