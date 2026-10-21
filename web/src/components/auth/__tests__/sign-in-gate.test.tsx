import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SignInGate } from "../sign-in-gate";
import { credentials, UNAUTHENTICATED_EVENT } from "@/lib/credentials";

function renderGate() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <SignInGate>
        <p>dashboard</p>
      </SignInGate>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
  credentials.setApiKey(null);
});

describe("SignInGate", () => {
  it("shows the dashboard until the server refuses the API key", () => {
    renderGate();
    expect(screen.getByText("dashboard")).toBeInTheDocument();
    act(() => {
      window.dispatchEvent(new CustomEvent(UNAUTHENTICATED_EVENT));
    });
    expect(screen.getByText("This server needs an API key")).toBeInTheDocument();
  });

  it("keeps a key only once the server accepts it", async () => {
    renderGate();
    act(() => {
      window.dispatchEvent(new CustomEvent(UNAUTHENTICATED_EVENT));
    });
    vi.stubGlobal("fetch", vi.fn(async () => new Response("{}", { status: 401 })));
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: "wrong" } });
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("did not accept");
    expect(credentials.apiKey()).toBeNull();

    vi.stubGlobal("fetch", vi.fn(async () => new Response("{}", { status: 200 })));
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: "right" } });
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    await waitFor(() => expect(screen.getByText("dashboard")).toBeInTheDocument());
    expect(credentials.apiKey()).toBe("right");
  });
});
