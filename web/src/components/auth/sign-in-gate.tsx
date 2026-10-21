import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { KeyRound } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { credentials, UNAUTHENTICATED_EVENT } from "@/lib/credentials";

/**
 * Shows the dashboard, or a sign-in screen when the server asks for an API key (it was
 * started with VALKA_API_KEYS and the stored key is missing or no longer valid).
 */
export function SignInGate({ children }: { children: React.ReactNode }) {
  const [needsKey, setNeedsKey] = useState(false);

  useEffect(() => {
    const onRefused = () => setNeedsKey(true);
    window.addEventListener(UNAUTHENTICATED_EVENT, onRefused);
    return () => window.removeEventListener(UNAUTHENTICATED_EVENT, onRefused);
  }, []);

  if (needsKey) return <SignIn onSignedIn={() => setNeedsKey(false)} />;
  return <>{children}</>;
}

export function SignIn({ onSignedIn }: { onSignedIn: () => void }) {
  const queryClient = useQueryClient();
  const [key, setKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const hadKey = credentials.apiKey() !== null;

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setChecking(true);
    try {
      // Check the key before keeping it.
      const res = await fetch("/api/v1/cluster", {
        headers: { Authorization: `Bearer ${key.trim()}` },
      });
      if (res.status === 401) {
        setError("The server did not accept this key. Check VALKA_API_KEYS on the server.");
        return;
      }
      if (!res.ok) {
        setError(`The server answered ${res.status}. Try again in a moment.`);
        return;
      }
      credentials.setApiKey(key.trim());
      await queryClient.invalidateQueries();
      onSignedIn();
    } catch {
      setError("Could not reach the server.");
    } finally {
      setChecking(false);
    }
  }

  return (
    <div className="flex min-h-[70vh] items-center justify-center">
      <form
        onSubmit={submit}
        className="w-full max-w-sm space-y-5 rounded-lg border bg-card p-6 shadow-sm"
        aria-labelledby="sign-in-title"
      >
        <div className="flex items-center gap-3">
          <div className="flex h-9 w-9 items-center justify-center rounded-md bg-primary/10 text-primary">
            <KeyRound className="h-4.5 w-4.5" />
          </div>
          <div>
            <h1 id="sign-in-title" className="text-base font-semibold text-foreground">
              {hadKey ? "Your API key was refused" : "This server needs an API key"}
            </h1>
            <p className="text-xs text-muted-foreground">
              Use one of the keys in the server's VALKA_API_KEYS.
            </p>
          </div>
        </div>
        <div className="space-y-2">
          <Label htmlFor="api-key">API key</Label>
          <Input
            id="api-key"
            type="password"
            autoComplete="off"
            autoFocus
            required
            value={key}
            onChange={(e) => setKey(e.target.value)}
          />
          <p className="text-xs text-muted-foreground">
            Kept in this browser until you sign out.
          </p>
        </div>
        {error && (
          <p role="alert" className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {error}
          </p>
        )}
        <Button type="submit" className="w-full" disabled={checking || key.trim() === ""}>
          {checking ? "Checking…" : "Sign in"}
        </Button>
      </form>
    </div>
  );
}
