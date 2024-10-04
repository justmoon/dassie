import { zodResolver } from "@hookform/resolvers/zod"
import { TriangleAlertIcon } from "lucide-react"
import { useMemo } from "react"
import { useForm } from "react-hook-form"
import { Redirect, useLocation } from "wouter"
import { z } from "zod"

import type { SettlementSchemeId } from "@dassie/app-dassie/src/peer-protocol/types/settlement-scheme-id"

import {
  Checklist,
  ChecklistItem,
} from "../../../components/ui-custom/checklist"
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "../../../components/ui/alert"
import { Button } from "../../../components/ui/button"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "../../../components/ui/card"
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "../../../components/ui/form"
import { Input } from "../../../components/ui/input"
import { Label } from "../../../components/ui/label"
import { SCHEME_NAME_MAP } from "../../../constants/schemes"
import { rpc } from "../../../utils/rpc"

interface DeleteLedgerAccountProperties {
  id: SettlementSchemeId
}

export function DeleteLedgerAccount({ id }: DeleteLedgerAccountProperties) {
  const [, navigate] = useLocation()

  const confirmationString = "DELETE " + id.toUpperCase()

  const { data: prerequisites } =
    rpc.ledgers.checkLedgerDeletePrerequisites.useQuery(id)

  const deleteLedgerAccount = rpc.ledgers.removeSettlementScheme.useMutation({
    onSuccess: () => {
      navigate("/ledgers")
    },
  })

  const formSchema = useMemo(
    () =>
      z.object({
        confirmation: z.literal(confirmationString, {
          errorMap: () => ({
            message: "Confirmation must match exactly the expected format",
          }),
        }),
      }),
    [confirmationString],
  )

  const form = useForm<z.infer<typeof formSchema>>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      confirmation: "",
    },
  })

  function onSubmit(values: z.infer<typeof formSchema>) {
    if (values.confirmation !== confirmationString) {
      throw new Error(
        "Confirmation invalid but should have already been validated",
      )
    }

    deleteLedgerAccount.mutate({ id })
  }

  if (!prerequisites) {
    return <div>Loading...</div>
  }

  if (!prerequisites.isActive) {
    return <Redirect to="/ledgers" />
  }

  const allPrerequisitesValid = prerequisites.isUnused

  return (
    <div className="flex h-full items-center justify-center">
      <Card className="w-full max-w-lg mx-8 h-full max-h-lg">
        <CardHeader>
          <CardTitle>Removing Ledger {SCHEME_NAME_MAP[id]}</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <Alert variant="destructive">
            <TriangleAlertIcon className="size-4" />
            <AlertTitle>Possibility to lose funds</AlertTitle>
            <AlertDescription>
              You are about to remove a ledger account. This will delete the
              keys from Dassie. Make sure the account is empty and/or you have
              backed up the keys.
            </AlertDescription>
          </Alert>

          <div>
            <Label>Prerequisites</Label>
            <Checklist className="flex-1">
              <ChecklistItem
                variant={prerequisites.isUnused ? "valid" : "invalid"}
              >
                {prerequisites.isUnused ?
                  "Not used by any peers"
                : "Settlement scheme is still in use for some peers"}
              </ChecklistItem>
            </Checklist>
          </div>

          <Form {...form}>
            <form
              onSubmit={(...parameters) =>
                void form.handleSubmit(onSubmit)(...parameters)
              }
              className="flex flex-col gap-4"
            >
              <FormField
                control={form.control}
                name="confirmation"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel htmlFor="confirmation">Confirmation</FormLabel>
                    <FormControl>
                      <Input {...field} />
                    </FormControl>
                    <FormDescription>
                      Enter &ldquo;{confirmationString}&rdquo; to confirm
                      deletion
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
              <Button
                type="submit"
                variant="destructive"
                disabled={!allPrerequisitesValid}
              >
                Remove Ledger Account
              </Button>
            </form>
          </Form>
        </CardContent>
      </Card>
    </div>
  )
}
