import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const badgeVariants = cva(
  "inline-flex h-5 items-center gap-1.5 rounded-none border px-1.5 font-mono text-[11px] uppercase tracking-[0.14em] [&_svg]:size-3",
  {
    variants: {
      variant: {
        neutral: "border-border text-muted-foreground",
        success: "border-border text-foreground",
        warning: "border-border text-foreground",
        destructive: "border-destructive/60 text-destructive",
      },
    },
    defaultVariants: { variant: "neutral" },
  },
);

export interface BadgeProps
  extends React.HTMLAttributes<HTMLSpanElement>,
    VariantProps<typeof badgeVariants> {}

export function Badge({ className, variant, ...props }: BadgeProps) {
  return <span className={cn(badgeVariants({ variant }), className)} {...props} />;
}
