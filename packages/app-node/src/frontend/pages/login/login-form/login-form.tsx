import { zodResolver } from "@hookform/resolvers/zod"
import * as bip39 from "@scure/bip39"
import { useMutation } from "@tanstack/react-query"
import { Loader2 as Loader } from "lucide-react"
import { HTMLAttributes, useCallback } from "react"
import { useForm } from "react-hook-form"
import { SetReturnType } from "type-fest"
import { z } from "zod"

import { Button } from "../../../components/ui/button"
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "../../../components/ui/form"
import { Input } from "../../../components/ui/input"
import { login } from "../../../utils/authentication"
import { combine } from "../../../utils/class-helper"
import { queryClientReactContext } from "../../../utils/trpc"

const formSchema = z.object({
  passphrase: z.string(),
})

export const LoginForm = ({
  className,
  ...properties
}: HTMLAttributes<HTMLDivElement>) => {
  const loginMutation = useMutation({
    mutationFn: async (passphrase: string) => {
      const seed = await bip39.mnemonicToSeed(passphrase)
      await login(seed)
    },
    onSuccess: () => {
      window.location.reload()
    },
    context: queryClientReactContext,
  })

  const onSubmit = useCallback(
    ({ passphrase }: z.infer<typeof formSchema>) => {
      loginMutation.mutate(passphrase)
    },
    [loginMutation]
  )

  const form = useForm<z.infer<typeof formSchema>>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      passphrase: "",
    },
  })

  return (
    <div className={combine("grid gap-6", className)} {...properties}>
      <Form {...form}>
        <form
          onSubmit={
            form.handleSubmit(onSubmit) as SetReturnType<
              ReturnType<typeof form.handleSubmit>,
              void
            >
          }
        >
          <div className="grid gap-2">
            <div className="grid gap-1">
              <FormField
                control={form.control}
                name="passphrase"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>Passphrase</FormLabel>
                    <FormControl>
                      <Input
                        placeholder="correct horse staple ..."
                        type="text"
                        autoCapitalize="none"
                        autoCorrect="off"
                        autoComplete="off"
                        disabled={loginMutation.isLoading}
                        {...field}
                      />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </div>
            <Button disabled={loginMutation.isLoading}>
              {loginMutation.isLoading && (
                <Loader className="mr-2 h-4 w-4 animate-spin" />
              )}
              Reconnect
            </Button>
          </div>
        </form>
      </Form>
    </div>
  )
}
