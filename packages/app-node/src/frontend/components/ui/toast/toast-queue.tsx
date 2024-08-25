import { ReactNode } from "react"
import { create } from "zustand"

import { ToastActionElement, ToastProperties } from "./toast"

const TOAST_LIMIT = 5
const TOAST_REMOVE_DELAY = 1_000_000

export type ToasterToast = ToastProperties & {
  id: string
  title?: ReactNode
  description?: ReactNode
  action?: ToastActionElement
}

export type NewToast = Omit<ToasterToast, "id">

interface ToastQueue {
  nextUniqueId: number
  toasts: ToasterToast[]
  addToast: (toast: NewToast) => ToasterToast
  updateToast: (toast: Partial<ToasterToast>) => void
  dismissToast: (toastId: ToasterToast["id"]) => void
  removeToast: (toastId: ToasterToast["id"]) => void
}

export const useToastQueue = create<ToastQueue>((set, get) => ({
  nextUniqueId: 0,
  toasts: [],
  addToast: (toast: NewToast) => {
    const { nextUniqueId, toasts } = get()
    const newToast: ToasterToast = {
      ...toast,
      id: String(nextUniqueId),
    }

    set({
      toasts: [newToast, ...toasts].slice(0, TOAST_LIMIT),
      nextUniqueId: (nextUniqueId + 1) % Number.MAX_SAFE_INTEGER,
    })

    return newToast
  },
  updateToast: (toast: Partial<ToasterToast>) => {
    set((state) => ({
      toasts: state.toasts.map((staleToast) =>
        staleToast.id === toast.id ? { ...staleToast, ...toast } : staleToast,
      ),
    }))
  },
  dismissToast: (toastId: ToasterToast["id"]) => {
    const toast = get().toasts.find((t) => t.id === toastId)

    if (!toast?.open) {
      return
    }

    setTimeout(() => {
      get().removeToast(toastId)
    }, TOAST_REMOVE_DELAY)

    set((state) => ({
      toasts: state.toasts.map((staleToast) =>
        staleToast.id === toastId ? { ...staleToast, open: false } : staleToast,
      ),
    }))
  },
  removeToast: (toastId: ToasterToast["id"]) => {
    set((state) => ({
      toasts: state.toasts.filter((staleToast) => staleToast.id !== toastId),
    }))
  },
}))
