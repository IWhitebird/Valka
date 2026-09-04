import { Card, CardContent } from "@/components/ui/card";
import { cn } from "@/lib/utils";

interface StatProps {
  label: string;
  value: React.ReactNode;
  hint?: React.ReactNode;
  tone?: "default" | "warn" | "bad";
}

export function Stat({ label, value, hint, tone = "default" }: StatProps) {
  return (
    <Card className="gap-0 py-0">
      <CardContent className="p-5">
        <p className="text-sm text-muted-foreground">{label}</p>
        <p
          className={cn(
            "mt-1 text-2xl font-semibold tracking-tight text-foreground",
            tone === "warn" && "text-amber-400",
            tone === "bad" && "text-red-400",
          )}
        >
          {value}
        </p>
        {hint && <p className="mt-1 text-xs text-muted-foreground">{hint}</p>}
      </CardContent>
    </Card>
  );
}
