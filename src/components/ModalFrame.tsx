import { useState, type ReactNode } from "react";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";
import { cn } from "@/lib/utils";
export { DialogTitle };
export function ModalFrame({
  children,
  onClose,
  className,
  width,
}: {
  children: ReactNode;
  onClose: () => void;
  className?: string;
  width?: number;
}) {
  const [opener] = useState(() =>
    document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null,
  );
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent
        style={{
          position: "fixed",
          left: "50%",
          top: "50%",
          transform: "translate(-50%, -50%)",
          width: width ?? (className?.includes("compact-modal") ? 480 : 570),
          maxWidth: "calc(100vw - 48px)",
          maxHeight: "calc(100dvh - 48px)",
          translate: "none",
        }}
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          if (opener?.isConnected) opener.focus();
        }}
        aria-describedby={undefined}
        showCloseButton={false}
        className={cn(
          "modal shadcn-dialog flex max-w-none translate-x-0 translate-y-0 flex-col gap-0 overflow-hidden rounded-[10px] p-0",
          className,
        )}
      >
        {children}
      </DialogContent>
    </Dialog>
  );
}
