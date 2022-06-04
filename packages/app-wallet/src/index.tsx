/* @refresh reload */
import { render } from "solid-js/web"
import "virtual:windi.css"

import App from "./app"
import "./index.css"
import Uplink from "./modules/uplink"

render(() => <App />, document.querySelector("#root") as HTMLElement)

new Uplink()
