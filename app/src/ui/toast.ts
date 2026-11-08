/** Small notices in the bottom centre: errors and one-off confirmations. */

export interface ToastOptions {
  /** Seconds before it goes away; 0 keeps it until dismissed. Errors stay by default. */
  timeout?: number;
  action?: { label: string; run: () => void };
}

export class Toasts {
  constructor(private readonly el: HTMLElement) {}

  show(message: string, kind: "info" | "error" = "info", opts: ToastOptions = {}): () => void {
    const toast = document.createElement("div");
    toast.className = `toast ${kind}`;
    toast.setAttribute("role", kind === "error" ? "alert" : "status");
    const text = document.createElement("span");
    text.textContent = message;
    toast.append(text);
    const dismiss = () => toast.remove();
    if (opts.action) {
      const { label, run } = opts.action;
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = label;
      button.addEventListener("click", () => {
        dismiss();
        run();
      });
      toast.append(button);
    }
    const close = document.createElement("button");
    close.type = "button";
    close.className = "close";
    close.setAttribute("aria-label", "Dismiss");
    close.textContent = "×";
    close.addEventListener("click", dismiss);
    toast.append(close);
    this.el.append(toast);
    const timeout = opts.timeout ?? (kind === "error" ? 0 : 6);
    if (timeout > 0) window.setTimeout(dismiss, timeout * 1000);
    return dismiss;
  }

  error(message: string, opts?: ToastOptions): () => void {
    return this.show(message, "error", opts);
  }
}

/** Error text from a failed Tauri command (a string) or anything else thrown. */
export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}
