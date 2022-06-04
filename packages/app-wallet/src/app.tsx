import type { Component } from "solid-js"

import logo from "./logo.svg"

const App: Component = () => {
  return (
    <div class="text-center">
      <header class="flex flex-col min-h-screen bg-gray-700 text-lg text-white items-center justify-center">
        <img
          src={logo}
          class="h-[40vmin] animate-spin animate-duration-8000 animated pointer-events-none"
          alt="logo"
        />
        <p>
          Edit <code>src/App.tsx</code> and save to reload.
        </p>
        <a
          class="text-red-600"
          href="https://github.com/solidjs/solid"
          target="_blank"
          rel="noopener noreferrer"
        >
          Learn Solid
        </a>
      </header>
    </div>
  )
}

export default App
