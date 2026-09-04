import { Link, useLocation } from "react-router-dom";
import { cn } from "@/lib/utils";

const TABS = [
  { name: "Overview", href: "/cluster" },
  { name: "Shards", href: "/cluster/shards" },
  { name: "Storage", href: "/cluster/storage" },
];

export function ClusterTabs() {
  const { pathname } = useLocation();
  return (
    <nav className="flex gap-1 border-b">
      {TABS.map((t) => {
        const active =
          t.href === "/cluster"
            ? pathname === "/cluster" || pathname.startsWith("/cluster/nodes")
            : pathname.startsWith(t.href);
        return (
          <Link
            key={t.href}
            to={t.href}
            className={cn(
              "-mb-px border-b-2 px-3 py-2 text-sm transition-colors",
              active
                ? "border-primary text-foreground"
                : "border-transparent text-muted-foreground hover:text-foreground",
            )}
          >
            {t.name}
          </Link>
        );
      })}
    </nav>
  );
}
