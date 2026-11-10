import * as React from "react";
import * as SwitchPrimitive from "@radix-ui/react-switch";
import { cn } from "@/lib/utils";

export const Switch = React.forwardRef<
  React.ElementRef<typeof SwitchPrimitive.Root>,
  React.ComponentPropsWithoutRef<typeof SwitchPrimitive.Root>
>(({ className, ...props }, ref) => (
  <SwitchPrimitive.Root
    ref={ref}
    className={cn(
      "peer inline-flex h-4 w-7 shrink-0 cursor-default items-center rounded-none border border-border bg-transparent transition-colors data-[state=checked]:bg-white disabled:cursor-not-allowed disabled:opacity-40",
      className,
    )}
    {...props}
  >
    <SwitchPrimitive.Thumb className="pointer-events-none block size-2.5 rounded-none bg-white transition-transform data-[state=checked]:translate-x-3 data-[state=checked]:bg-[#050505]" />
  </SwitchPrimitive.Root>
));
Switch.displayName = "Switch";
