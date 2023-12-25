import {
  Toast,
  ToastClose,
  ToastDescription,
  ToastProvider,
  ToastTitle,
  ToastViewport,
} from "./toast"
import { useToastQueue } from "./toast-queue"

export function Toaster() {
  const toasts = useToastQueue((state) => state.toasts)

  return (
    <ToastProvider>
      {toasts.map(function ({ id, title, description, action, ...properties }) {
        return (
          <Toast key={id} {...properties}>
            <div className="grid gap-1">
              {title && <ToastTitle>{title}</ToastTitle>}
              {description && (
                <ToastDescription>{description}</ToastDescription>
              )}
            </div>
            {action}
            <ToastClose />
          </Toast>
        )
      })}
      <ToastViewport />
    </ToastProvider>
  )
}
