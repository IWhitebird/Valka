import { act, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { credentials } from "@/lib/credentials";
import { Sidebar } from "../sidebar";

afterEach(() => {
  vi.unstubAllGlobals();
  credentials.setApiKey(null);
  credentials.setAdminToken(null);
});

describe("Sidebar", () => {
  it("offers Sign out as soon as a key is stored, and hides it once signed out", () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("{}", { status: 503 })));
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <MemoryRouter>
          <Sidebar />
        </MemoryRouter>
      </QueryClientProvider>,
    );
    expect(screen.queryByRole("button", { name: "Sign out" })).not.toBeInTheDocument();

    // As the sign-in screen does, outside the sidebar.
    act(() => credentials.setApiKey("k"));
    fireEvent.click(screen.getByRole("button", { name: "Sign out" }));
    expect(credentials.apiKey()).toBeNull();
    expect(screen.queryByRole("button", { name: "Sign out" })).not.toBeInTheDocument();
  });
});
