import { Outlet } from "react-router-dom";
import { SignInGate } from "@/components/auth/sign-in-gate";
import { Sidebar } from "./sidebar";

export function RootLayout() {
  return (
    <div className="flex h-screen overflow-hidden bg-background">
      <Sidebar />
      <main className="flex-1 overflow-y-auto">
        <div className="mx-auto max-w-7xl px-8 py-8">
          <SignInGate>
            <Outlet />
          </SignInGate>
        </div>
      </main>
    </div>
  );
}
