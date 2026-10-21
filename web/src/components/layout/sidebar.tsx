import { Link, useLocation } from "react-router-dom";
import {
  LayoutDashboard,
  ListTodo,
  Users,
  Activity,
  AlertTriangle,
  Server,
} from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { cn } from "@/lib/utils";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import { useClusterOverview } from "@/hooks/use-cluster";
import { useHasApiKey } from "@/hooks/use-credentials";
import { credentials } from "@/lib/credentials";

const navigation = [
  { name: "Dashboard", href: "/", icon: LayoutDashboard },
  { name: "Tasks", href: "/tasks", icon: ListTodo },
  { name: "Workers", href: "/workers", icon: Users },
  { name: "Cluster", href: "/cluster", icon: Server },
  { name: "Events", href: "/events", icon: Activity },
  { name: "Dead Letters", href: "/dead-letters", icon: AlertTriangle },
];

export function Sidebar() {
  const location = useLocation();
  const queryClient = useQueryClient();
  const { data: overview } = useClusterOverview();
  const signedIn = useHasApiKey();
  const version = overview?.nodes.find((n) => n.node_id === overview.this_node)?.version;

  function signOut() {
    credentials.setApiKey(null);
    credentials.setAdminToken(null);
    queryClient.invalidateQueries();
  }

  function isActive(href: string): boolean {
    if (href === "/") return location.pathname === "/";
    return location.pathname.startsWith(href);
  }

  return (
    <aside className="flex h-screen w-56 flex-col border-r bg-background">
      <div className="flex h-14 items-center gap-2.5 px-5">
        <Link to="/" className="flex items-center gap-2.5">
          <img src="/valka.svg" alt="" className="h-6 w-6" />
          <span className="font-display text-[19px] font-extrabold tracking-tight text-foreground">
            valka
          </span>
        </Link>
      </div>

      <Separator />

      <nav className="flex-1 space-y-0.5 px-2.5 py-3">
        <p className="mb-2 px-2.5 text-[11px] font-medium uppercase tracking-widest text-muted-foreground/60">
          Navigation
        </p>
        {navigation.map((item) => {
          const active = isActive(item.href);
          return (
            <Link
              key={item.name}
              to={item.href}
              className={cn(
                "flex items-center gap-2.5 rounded-md px-2.5 py-1.5 text-[13px] font-medium transition-colors",
                active
                  ? "bg-primary/10 text-primary"
                  : "text-muted-foreground hover:bg-accent hover:text-foreground",
              )}
            >
              <item.icon className={cn("h-4 w-4", active && "text-primary")} />
              {item.name}
            </Link>
          );
        })}
      </nav>

      <Separator />

      <div className="space-y-2 px-5 py-3">
        <div className="flex items-center justify-between">
          <span className="truncate text-[11px] text-muted-foreground/70" title="The node serving this dashboard">
            {overview?.this_node ?? "Valka"}
          </span>
          {version && (
            <Badge variant="secondary" className="h-5 text-[10px] font-normal text-muted-foreground">
              v{version}
            </Badge>
          )}
        </div>
        {signedIn && (
          <button
            type="button"
            onClick={signOut}
            className="text-[11px] text-muted-foreground/70 underline-offset-2 hover:text-foreground hover:underline"
          >
            Sign out
          </button>
        )}
      </div>
    </aside>
  );
}
