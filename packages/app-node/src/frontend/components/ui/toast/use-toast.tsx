import { NewToast, ToasterToast, useToastQueue } from "./toast-queue"

function toast(newToast: NewToast) {
  const { addToast, updateToast, dismissToast } = useToastQueue.getState()

  const { id } = addToast({
    ...newToast,
    open: true,

    onOpenChange: (open) => {
      if (!open) dismiss()
    },
  })

  const update = (properties: Partial<ToasterToast>) => {
    updateToast({ ...properties, id })
  }
  const dismiss = () => {
    dismissToast(id)
  }

  return {
    id,
    dismiss,
    update,
  }
}

function useToast() {
  const dismissToast = useToastQueue((state) => state.dismissToast)

  return {
    toast,
    dismiss: dismissToast,
  }
}

export { useToast, toast }
