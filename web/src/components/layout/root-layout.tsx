import { useEffect } from "react";
import { Outlet, useLocation } from "react-router-dom";
import { SignInGate } from "@/components/auth/sign-in-gate";
import { partialAnswers } from "@/lib/partial";
import { PartialBanner } from "./partial-banner";
import { Sidebar } from "./sidebar";

export function RootLayout() {
  const { pathname } = useLocation();

  // A new page asks its own questions: forget partial answers of the previous one.
  useEffect(() => {
    partialAnswers.clear();
  }, [pathname]);

  return (
    <div className="flex h-screen overflow-hidden bg-background">
      <Sidebar />
      <main className="flex-1 overflow-y-auto">
        <div className="mx-auto max-w-7xl px-8 py-8">
          <PartialBanner />
          <SignInGate>
            <Outlet />
          </SignInGate>
        </div>
      </main>
    </div>
  );
}
