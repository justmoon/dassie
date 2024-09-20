import { Logo } from "../../components/vectors/logo"
import { LoginForm } from "./login-form/login-form"

export const LoginPage = () => {
  return (
    <div className="container relative h-full flex-col items-center justify-center grid lg:max-w-none lg:grid-cols-2 lg:px-0">
      <div className="relative hidden h-full flex-col bg-muted p-10 text-white dark:border-r lg:flex">
        <div className="absolute inset-0 bg-zinc-900" />
        <div className="relative z-20 flex items-center text-lg font-medium">
          <Logo className="h-6 w-6 mr-2" />
          Dassie Wallet
        </div>
        <div className="relative z-20 mt-auto">
          <blockquote className="space-y-2">
            <p className="text-lg">
              &ldquo;Economic control is not merely control of a sector of
              <br />
              human life which can be separated from the rest;
              <br />
              it is the control of the means for all our ends.&rdquo;
            </p>
            <p className="text-sm text-muted-foreground">F.A.Hayek</p>
          </blockquote>
        </div>
      </div>
      <div className="lg:p-8">
        <div className="mx-auto flex w-full flex-col justify-center space-y-6 sm:w-[350px]">
          <div className="flex flex-col space-y-2 text-center">
            <h1 className="text-2xl font-semibold tracking-tight">
              Reconnect to your Dassie instance
            </h1>
            <p className="text-sm text-muted-foreground">
              Enter your wallet passphrase
            </p>
          </div>
          <LoginForm />
          <div className="relative">
            <div className="absolute inset-0 flex items-center">
              <span className="w-full border-t" />
            </div>
          </div>
          <footer className="px-8 text-center text-sm text-muted-foreground">
            <a href="https://dassie.land">What is Dassie?</a>
          </footer>
        </div>
      </div>
    </div>
  )
}
